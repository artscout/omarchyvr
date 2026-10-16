//! The local model of the AI tab: `pimax-ai` (llama.cpp `llama-server` with Qwen3.5, docs/AI.md 1 and 10).
//!
//! The status is `pimax-ai status --json`, run in the background every 5 s while the tab is open (never on the UI
//! thread). Install, Start (GPU or CPU), Stop and Test are jobs (Start in its own user scope, so the server outlives
//! the app). "Use local model" adds the provider voice control needs: openai-compatible,
//! `http://127.0.0.1:8080/v1`, model `pimax-local`. Starting on the GPU while a VR session runs is allowed: the page
//! shows the 4 GiB budget next to it.
use crate::ai::KeyChoice;
use crate::app::AppCore;
use crate::cmd::Cmd;
use crate::job::{JobKind, JobSpec, Step};
use pimax_config::ai::{Feature, Kind, Provider};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The local server's base URL.
pub const LOCAL_BASE: &str = "http://127.0.0.1:8080/v1";
/// The model alias the server answers to.
pub const LOCAL_MODEL: &str = "pimax-local";
/// What Install downloads.
pub const INSTALL_SIZE: &str = "4.1 GB";
/// The job kinds.
pub const KIND: &str = "localai";
/// The test job's kind.
pub const TEST_KIND: &str = "localai-test";

/// One model file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalModel {
    /// `model-gpu`, `model-cpu`.
    pub id: String,
    /// The GGUF file.
    pub file: String,
    /// Downloaded.
    pub installed: bool,
}

/// The running server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Running {
    /// Pid.
    pub pid: u32,
    /// Port.
    pub port: u16,
    /// "gpu (all layers)", "cpu", ...
    pub mode: String,
    /// The model file.
    pub model: String,
    /// Its OpenAI-compatible base URL.
    pub url: String,
}

/// `pimax-ai status --json`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalAiStatus {
    /// Runtime and models are there.
    pub installed: bool,
    /// The llama-server binary.
    pub runtime: Option<String>,
    /// The model files.
    pub models: Vec<LocalModel>,
    /// The server, when it runs.
    pub running: Option<Running>,
    /// `/health` answered: the model is loaded.
    pub ready: bool,
    /// VRAM in use (None in CPU mode or without nvidia-smi).
    pub gpu_mib: Option<u64>,
    /// The VRAM budget.
    pub gpu_budget_mib: Option<u64>,
    /// The model alias (`pimax-local`).
    pub model_alias: Option<String>,
}

impl LocalAiStatus {
    /// Running on the GPU.
    pub fn on_gpu(&self) -> bool {
        self.running.as_ref().is_some_and(|r| r.mode.starts_with("gpu"))
    }
}

/// Parse the one-line JSON.
pub fn parse_status(text: &str) -> Result<LocalAiStatus, String> {
    let line = text.lines().rev().find(|l| l.trim_start().starts_with('{')).ok_or("pimax-ai status gave no JSON")?;
    let v: Value = serde_json::from_str(line.trim()).map_err(|e| format!("pimax-ai status: {e}"))?;
    let s = |x: &Value| x.as_str().map(String::from);
    Ok(LocalAiStatus {
        installed: v["installed"].as_bool().unwrap_or(false),
        runtime: s(&v["runtime"]),
        models: v["models"]
            .as_array()
            .map(|a| a.iter().map(|m| LocalModel { id: s(&m["id"]).unwrap_or_default(), file: s(&m["file"]).unwrap_or_default(), installed: m["installed"].as_bool().unwrap_or(false) }).collect())
            .unwrap_or_default(),
        running: v["running"].as_object().map(|r| Running {
            pid: r.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32,
            port: r.get("port").and_then(Value::as_u64).unwrap_or(0) as u16,
            mode: r.get("mode").and_then(Value::as_str).unwrap_or("").to_string(),
            model: r.get("model").and_then(Value::as_str).unwrap_or("").to_string(),
            url: r.get("url").and_then(Value::as_str).unwrap_or("").to_string(),
        }),
        ready: v["ready"].as_bool().unwrap_or(false),
        gpu_mib: v["gpu_mib"].as_u64(),
        gpu_budget_mib: v["gpu_budget_mib"].as_u64(),
        model_alias: s(&v["model_alias"]),
    })
}

/// The demo status: installed, running on the GPU, ready.
pub fn demo_status() -> LocalAiStatus {
    LocalAiStatus {
        installed: true,
        runtime: Some("~/.local/share/pimax/ai/runtime/runtime-vulkan-b11630/llama-b11630/llama-server".into()),
        models: vec![
            LocalModel { id: "model-gpu".into(), file: "Qwen3.5-4B-Q4_K_M.gguf".into(), installed: true },
            LocalModel { id: "model-cpu".into(), file: "Qwen3.5-2B-Q4_K_M.gguf".into(), installed: true },
        ],
        running: Some(Running { pid: 4321, port: 8080, mode: "gpu (all layers)".into(), model: "Qwen3.5-4B-Q4_K_M.gguf".into(), url: LOCAL_BASE.into() }),
        ready: true,
        gpu_mib: Some(3480),
        gpu_budget_mib: Some(4096),
        model_alias: Some(LOCAL_MODEL.into()),
    }
}

/// Where `pimax-ai` may be: the workspace build, cargo's bin, ~/.local/bin, a package.
pub fn candidates(release: &Path, home: &Path) -> Vec<PathBuf> {
    vec![release.join("pimax-ai"), home.join(".cargo/bin/pimax-ai"), home.join(".local/bin/pimax-ai"), PathBuf::from("/usr/bin/pimax-ai")]
}

/// `pimax-ai status --json` with a deadline (background only). A status that hangs is left to finish by itself.
fn run_status(bin: &Path) -> Result<LocalAiStatus, String> {
    crate::cmd::allow_start("pimax-ai status")?;
    let mut child = std::process::Command::new(bin)
        .args(["status", "--json"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("{}: {e}", bin.display()))?;
    let out = child.stdout.take();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(mut o) = out {
            use std::io::Read;
            let _ = o.read_to_string(&mut s);
        }
        let _ = child.wait();
        let _ = tx.send(s);
    });
    let text = rx.recv_timeout(Duration::from_secs(8)).map_err(|_| "pimax-ai status did not answer within 8 s".to_string())?;
    parse_status(&text)
}

/// The local model's part of the AI tab.
#[derive(Debug, Clone, Default)]
pub struct LocalAiView {
    /// The `pimax-ai` binary found (None: not built or not looked for yet).
    pub bin: Option<PathBuf>,
    /// The last status, or why there is none.
    pub status: Option<Result<LocalAiStatus, String>>,
    /// The last Test's line ("<ms> ms: <reply>") or its error.
    pub test: Option<Result<String, String>>,
    at: Option<Instant>,
    test_from: Option<usize>,
}

impl AppCore {
    /// Keep the status fresh (every 5 s while the tab is open; in the background).
    pub fn want_localai(&mut self) {
        if self.localai.at.is_some_and(|t| t.elapsed() < Duration::from_secs(5)) || self.task_busy("localai") {
            return;
        }
        self.localai.at = Some(Instant::now());
        if self.demo {
            self.localai.bin = Some(self.paths.bin("pimax-ai"));
            self.localai.status = Some(Ok(demo_status()));
            return;
        }
        let cands = candidates(&self.paths.release, &self.paths.home);
        self.task(Some("localai"), move || {
            let bin = cands.into_iter().find(|p| p.is_file());
            let st = match &bin {
                Some(b) => run_status(b),
                None => Err("pimax-ai is not built yet".into()),
            };
            Box::new(move |c: &mut AppCore| {
                c.localai.bin = bin;
                c.localai.status = Some(st);
            })
        });
    }

    fn localai_spawn(&mut self, args: &[&str], label: &str, own_scope: bool, kind: &str) {
        let Some(bin) = self.localai.bin.clone() else {
            self.message = "pimax-ai is not built yet (cargo build --release -p pimax-ai)".into();
            return;
        };
        let mut c = Cmd::new(bin.display().to_string(), args);
        if own_scope && !self.demo {
            // the server outlives the app (its own user scope, like a desk start)
            c = self.own_scope(c, "ai");
        }
        if self.spawn(JobSpec { kind: JobKind::Other(kind.into()), label: label.into(), steps: vec![Step::Run(c)] }) {
            self.localai.at = None;
            if kind == TEST_KIND {
                self.localai.test_from = Some(self.log.len());
            }
        }
    }

    /// Build `pimax-ai` in the workspace (a job; the status finds it afterwards).
    pub fn localai_build(&mut self) {
        let c = Cmd::new("cargo", &["build", "--release", "-p", "pimax-ai"]).cwd(&self.paths.rust);
        if self.spawn(JobSpec { kind: JobKind::Other(KIND.into()), label: "Build pimax-ai".into(), steps: vec![Step::Run(c)] }) {
            self.localai.at = None;
        }
    }

    /// Download the runtime and models (4.1 GB; resumes when run again).
    pub fn localai_install(&mut self) {
        self.localai_spawn(&["install"], &format!("Install the local model ({INSTALL_SIZE})"), false, KIND);
    }

    /// Start the server on the GPU or the CPU (returns once it is ready or still loading).
    pub fn localai_start(&mut self, gpu: bool) {
        if gpu {
            self.localai_spawn(&["serve"], "Start the local model on the GPU", true, KIND);
        } else {
            self.localai_spawn(&["serve", "--cpu"], "Start the local model on the CPU", true, KIND);
        }
    }

    /// Stop the server.
    pub fn localai_stop(&mut self) {
        self.localai_spawn(&["stop"], "Stop the local model", false, KIND);
    }

    /// One tiny request to the running server (prints the latency).
    pub fn localai_test(&mut self) {
        self.localai.test = None;
        self.localai_spawn(&["test"], "Test the local model", false, TEST_KIND);
    }

    /// A local-model job ended (the status is asked again; a test's line is kept).
    pub(crate) fn localai_done(&mut self, kind: &str, ok: bool) {
        self.localai.at = None;
        if kind == TEST_KIND {
            let from = self.localai.test_from.take().unwrap_or(0).min(self.log.len());
            let line = self.log[from..].iter().rev().find(|l| l.contains(" ms: ")).cloned();
            self.localai.test = Some(match (line, ok) {
                (Some(l), true) => Ok(l.trim().to_string()),
                (_, _) => Err("The test did not answer (see the output below).".into()),
            });
        }
    }

    /// Why starting on the GPU needs a second look (a VR session runs): the budget next to it.
    pub fn localai_gpu_warning(&self) -> Option<String> {
        self.live.service.as_ref()?;
        let budget = self.localai.status.as_ref().and_then(|s| s.as_ref().ok()).and_then(|s| s.gpu_budget_mib).unwrap_or(4096);
        Some(format!("A VR session runs: the model takes up to {budget} MiB of VRAM next to it (allowed; Start on the CPU uses none)."))
    }

    /// Is a provider for the local server there (any name)?
    pub fn localai_provider(&self) -> Option<String> {
        self.ai.config.providers.iter().find(|p| p.kind == Kind::OpenAiCompatible && p.base().is_some_and(|b| b.trim_end_matches('/') == LOCAL_BASE)).map(|p| p.name.clone())
    }

    /// One click: the provider for the local server (openai-compatible, 127.0.0.1:8080/v1, pimax-local), used by
    /// voice control.
    pub fn localai_use(&mut self) -> Result<(), String> {
        let name = match self.localai_provider() {
            Some(n) => n,
            None => {
                let mut name = "local".to_string();
                let mut n = 2;
                while self.ai.config.provider(&name).is_some() {
                    name = format!("local-{n}");
                    n += 1;
                }
                let mut p = Provider::new(&name, Kind::OpenAiCompatible);
                p.base_url = Some(LOCAL_BASE.into());
                p.model = LOCAL_MODEL.into();
                self.ai_put(None, p, KeyChoice::None)?;
                name
            }
        };
        self.ai_set_use(Feature::Voice, Some(&name));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_line() {
        let j = r#"{"installed":true,"runtime":"/r/llama-server","models":[{"id":"model-gpu","file":"Qwen3.5-4B-Q4_K_M.gguf","installed":true},{"id":"model-cpu","file":"Qwen3.5-2B-Q4_K_M.gguf","installed":false}],"running":{"pid":1234,"port":8080,"mode":"gpu (all layers)","model":"Qwen3.5-4B-Q4_K_M.gguf","url":"http://127.0.0.1:8080/v1"},"ready":true,"gpu_mib":3480,"gpu_budget_mib":4096,"model_alias":"pimax-local"}"#;
        let s = parse_status(&format!("some note\n{j}\n")).unwrap();
        assert!(s.installed && s.ready && s.on_gpu());
        assert_eq!((s.gpu_mib, s.gpu_budget_mib, s.models.len(), s.models[1].installed), (Some(3480), Some(4096), 2, false));
        assert_eq!(s.running.as_ref().unwrap().pid, 1234);
        let stopped = parse_status(r#"{"installed":false,"runtime":null,"models":[],"running":null,"ready":false,"gpu_mib":null,"gpu_budget_mib":4096,"model_alias":"pimax-local"}"#).unwrap();
        assert!(stopped.running.is_none() && stopped.gpu_mib.is_none() && !stopped.on_gpu());
        assert!(parse_status("error: not installed").is_err());
    }

    #[test]
    fn use_local_model_adds_the_provider_once_and_picks_it_for_voice() {
        let d = std::env::temp_dir().join(format!("pimax-localai-{}", std::process::id()));
        let mut a = AppCore::demo(true, pimax_config::Store::new(d.join("a"), d.join("b")));
        a.want_ai();
        a.want_localai();
        assert!(matches!(&a.localai.status, Some(Ok(s)) if s.ready));
        assert!(a.localai_gpu_warning().unwrap().contains("4096 MiB"), "a VR session runs in this demo");
        let before = a.ai.config.providers.len();
        a.localai_use().unwrap();
        a.localai_use().unwrap();
        let p = a.ai.config.providers.iter().find(|p| p.base() == Some(LOCAL_BASE)).expect("the local provider");
        assert_eq!((p.model.as_str(), p.kind), (LOCAL_MODEL, Kind::OpenAiCompatible));
        assert_eq!(a.ai.config.providers.len(), before + 1, "added once");
        assert_eq!(a.ai_resolve(Feature::Voice).unwrap().model, LOCAL_MODEL);
        // the buttons only run jobs (demo: nothing starts)
        a.localai_start(true);
        assert!(a.message.contains("demo mode: would run Start the local model on the GPU") && a.job.is_none());
        let _ = std::fs::remove_dir_all(d);
    }
}
