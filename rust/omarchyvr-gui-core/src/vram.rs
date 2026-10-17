//! "Graphics card: free for the game" while Play runs (docs/PLAY-WORK-AUDIT.md). Read-only and without work on the GPU:
//! who holds video memory now (`nvidia-smi --query-compute-apps` / `--query-gpu`), which of those are ours (the local
//! AI, the speech server, voice, the Work desk; by their command lines in /proc), and whether a local AI server is still
//! running at all (its pid files: on the processor it holds no video memory, but it must not run next to a game
//! either). Nothing is stopped from here: Play's start stops Work and the AI by pid; this shows that it worked.
use crate::app::AppCore;
use crate::machine::Machine;
use std::path::Path;
use std::time::{Duration, Instant};

/// One process holding video memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuUser {
    pub pid: u32,
    /// nvidia-smi's process name (a path or a name).
    pub name: String,
    pub mib: Option<u32>,
    /// What of ours it is, in plain words (None: not ours, e.g. the game, Sunshine, a browser).
    pub ours: Option<&'static str>,
}

/// The check.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VramReport {
    pub users: Vec<GpuUser>,
    /// Local AI servers still running by their pid files (name, pid), on the GPU or not.
    pub ai_running: Vec<(String, u32)>,
    pub free_mib: Option<u32>,
    pub total_mib: Option<u32>,
    /// Why the graphics card could not be asked (no nvidia-smi: another GPU, or no driver tools).
    pub error: Option<String>,
}

/// `pid, process_name, used_memory` lines of `nvidia-smi --query-compute-apps=... --format=csv,noheader,nounits`.
pub fn parse_apps(csv: &str) -> Vec<(u32, String, Option<u32>)> {
    csv.lines()
        .filter_map(|l| {
            let mut f = l.split(',').map(str::trim);
            let pid = f.next()?.parse().ok()?;
            let name = f.next().unwrap_or("").to_string();
            let mib = f.next().and_then(|m| m.parse().ok());
            Some((pid, name, mib))
        })
        .collect()
}

/// `memory.free, memory.total` of the first GPU.
pub fn parse_gpu(csv: &str) -> Option<(u32, u32)> {
    let l = csv.lines().next()?;
    let (f, t) = l.split_once(',')?;
    Some((f.trim().parse().ok()?, t.trim().parse().ok()?))
}

/// What of ours a command line is (Work's parts and the local AI), else None. Play's own wrist readout (a headless
/// WayVR) and Monado are not Work's.
pub fn ours(cmdline: &str) -> Option<&'static str> {
    let first = cmdline.split([' ', '\0']).next().unwrap_or("");
    let base = first.rsplit('/').next().unwrap_or(first);
    if cmdline.contains("llama-server") {
        Some("the local AI model")
    } else if cmdline.contains("laya-serve") {
        Some("Lara")
    } else if cmdline.contains("whisper-server") {
        Some("the speech server")
    } else if base.starts_with("omarchyvr-voice") {
        Some("voice")
    } else if base == "omarchyvr-passthrough" {
        Some("passthrough (Work)")
    } else if base == "wayvr" && !cmdline.contains("--headless") {
        Some("the Work desk (WayVR)")
    } else {
        None
    }
}

impl VramReport {
    /// Work's parts or the AI that still hold video memory or still run.
    pub fn work_left(&self) -> Vec<String> {
        let mut v: Vec<String> = self.users.iter().filter_map(|u| u.ours.map(|o| format!("{o} (pid {}{})", u.pid, u.mib.map(|m| format!(", {:.1} GB", m as f64 / 1024.0)).unwrap_or_default()))).collect();
        for (n, pid) in &self.ai_running {
            if !self.users.iter().any(|u| u.pid == *pid) {
                v.push(format!("{n} (pid {pid}, on the processor)"));
            }
        }
        v
    }

    /// The Home line: Some(true) = all free for the game, Some(false) = something of Work is left, None = unknown.
    pub fn line(&self) -> (Option<bool>, String) {
        let left = self.work_left();
        if !left.is_empty() {
            return (Some(false), format!("Graphics card: still in use by {}. Stop Play, then press Play again (it stops them first).", left.join(", ")));
        }
        if let Some(e) = &self.error {
            return (None, format!("Graphics card: not checked ({e})"));
        }
        let free = match (self.free_mib, self.total_mib) {
            (Some(f), Some(t)) => format!(": {:.1} of {:.1} GB free", f as f64 / 1024.0, t as f64 / 1024.0),
            _ => String::new(),
        };
        (Some(true), format!("Graphics card: nothing of Work or the AI uses it{free}"))
    }

    /// Other programs holding video memory (for the hover text; never touched).
    pub fn others(&self) -> Vec<String> {
        self.users.iter().filter(|u| u.ours.is_none()).map(|u| format!("{} (pid {}{})", u.name.rsplit('/').next().unwrap_or(&u.name), u.pid, u.mib.map(|m| format!(", {m} MiB")).unwrap_or_default())).collect()
    }
}

/// Ask the graphics card (read-only commands of the machine's allowlist) and /proc. `ai_running`: the local AI's
/// servers by their pid files.
pub fn collect(m: &dyn Machine, ai_running: Vec<(String, u32)>) -> VramReport {
    let mut r = VramReport { ai_running, ..Default::default() };
    match m.run("nvidia-smi", &["--query-compute-apps=pid,process_name,used_memory", "--format=csv,noheader,nounits"]) {
        Some(o) if o.code == Some(0) => {
            for (pid, name, mib) in parse_apps(&o.stdout) {
                let cmd = m.read_str(Path::new(&format!("/proc/{pid}/cmdline"))).unwrap_or_default();
                let what = ours(if cmd.is_empty() { &name } else { &cmd });
                r.users.push(GpuUser { pid, name, mib, ours: what });
            }
        }
        Some(o) => r.error = Some(format!("nvidia-smi: {}", o.stderr.lines().next().unwrap_or("failed").trim())),
        None => r.error = Some("nvidia-smi is not available".into()),
    }
    if let Some(o) = m.run("nvidia-smi", &["--query-gpu=memory.free,memory.total", "--format=csv,noheader,nounits"]).filter(|o| o.code == Some(0)) {
        if let Some((f, t)) = parse_gpu(&o.stdout) {
            r.free_mib = Some(f);
            r.total_mib = Some(t);
        }
    }
    r
}

/// The local AI's servers that run now (their pid files; background only).
pub fn ai_running(dirs: &omarchyvr_ai::Dirs) -> Vec<(String, u32)> {
    use omarchyvr_ai::Slot;
    [(Slot::Main, "the local AI model"), (Slot::Router, "Lara"), (Slot::Asr, "the speech server"), (Slot::Agent, "the agent model")]
        .iter()
        .filter_map(|(s, n)| omarchyvr_ai::running_slot(dirs, *s).map(|p| (n.to_string(), p.pid as u32)))
        .collect()
}

/// How often the check runs while Play runs.
pub const EVERY: Duration = Duration::from_secs(10);

impl AppCore {
    /// Keep the graphics-card check fresh while Play runs (every 10 s, in the background; demo: a fixed answer).
    pub fn want_vram(&mut self) {
        if self.vram.as_ref().is_some_and(|(t, _)| t.elapsed() < EVERY) || self.task_busy("vram") {
            return;
        }
        if self.demo {
            self.vram = Some((Instant::now(), demo_report()));
            return;
        }
        let m = self.machine.clone();
        let env = self.env.clone();
        self.task(Some("vram"), move || {
            let ai = ai_running(&omarchyvr_ai::Dirs::from_env(&|k| env(k)));
            let r = collect(m.as_ref(), ai);
            Box::new(move |c: &mut AppCore| c.vram = Some((Instant::now(), r)))
        });
    }
}

/// Demo mode's check: a game and Sunshine on the card, nothing of Work.
pub fn demo_report() -> VramReport {
    VramReport {
        users: vec![
            GpuUser { pid: 4242, name: "/home/u/.local/share/Steam/steamapps/common/Beat Saber/Beat Saber.exe".into(), mib: Some(5120), ours: None },
            GpuUser { pid: 1500, name: "/usr/bin/sunshine".into(), mib: Some(310), ours: None },
        ],
        ai_running: Vec::new(),
        free_mib: Some(17_400),
        total_mib: Some(24_564),
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_compute_list_and_who_is_ours() {
        let csv = "4242, /home/u/.local/share/Steam/steamapps/common/Game/game.exe, 5120\n77, /home/u/.local/share/omarchyvr/ai/runtime/runtime-vulkan-b11630/llama-b11630/llama-server, 3006\n88, [Not Found], 835\nbad line\n";
        let apps = parse_apps(csv);
        assert_eq!(apps.len(), 3);
        assert_eq!(apps[1], (77, "/home/u/.local/share/omarchyvr/ai/runtime/runtime-vulkan-b11630/llama-b11630/llama-server".into(), Some(3006)));
        assert_eq!(parse_gpu("17400, 24564\n"), Some((17400, 24564)));
        assert_eq!(ours("/x/llama-server -m m.gguf --port 8080"), Some("the local AI model"));
        assert_eq!(ours("nice -n 19 ionice -c 2 -n 7 /x/whisper-server -m /m"), Some("the speech server"));
        assert_eq!(ours("/v/bin/python3 /v/bin/laya-serve"), Some("Lara"));
        assert_eq!(ours("/home/u/.local/bin/omarchyvr-voice listen"), Some("voice"));
        assert_eq!(ours("/home/u/.local/lib/omarchyvr/wayvr/wayvr --openxr --show"), Some("the Work desk (WayVR)"));
        assert_eq!(ours("/home/u/.local/lib/omarchyvr/wayvr/wayvr --openxr --headless --show"), None, "Play's own wrist readout");
        assert_eq!(ours("/usr/bin/sunshine"), None, "never ours");
        assert_eq!(ours("/b/monado-service"), None, "Play's runtime itself");
    }

    #[test]
    fn the_line_says_free_or_names_what_is_left() {
        let ok = demo_report();
        let (s, l) = ok.line();
        assert_eq!(s, Some(true));
        assert!(l.contains("nothing of Work or the AI") && l.contains("17.0 of 24.0 GB free"), "{l}");
        assert_eq!(ok.others().len(), 2);
        let mut bad = demo_report();
        bad.users.push(GpuUser { pid: 77, name: "llama-server".into(), mib: Some(3006), ours: Some("the local AI model") });
        bad.ai_running.push(("Lara".into(), 78));
        let (s, l) = bad.line();
        assert_eq!(s, Some(false));
        assert!(l.contains("the local AI model (pid 77, 2.9 GB)") && l.contains("Lara (pid 78, on the processor)"), "{l}");
        let unknown = VramReport { error: Some("nvidia-smi is not available".into()), ..Default::default() };
        assert_eq!(unknown.line().0, None);
    }
}
