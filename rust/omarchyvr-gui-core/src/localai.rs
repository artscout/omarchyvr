//! The local model of the AI tab: `omarchyvr-ai` (llama.cpp `llama-server` with Qwen3.5, docs/AI.md 1 and 10).
//!
//! The status is `omarchyvr-ai status --json`, run in the background every 5 s while the tab is open (never on the UI
//! thread). Install, Start (GPU or CPU), Stop and Test are jobs (Start in its own user scope, so the server outlives
//! the app). "Use local model" adds the provider voice control needs: openai-compatible,
//! `http://127.0.0.1:8080/v1`, model `pimax-local`. Starting on the GPU while a VR session runs is allowed: the page
//! shows the 4 GiB budget next to it.
use crate::ai::KeyChoice;
use crate::app::AppCore;
use crate::cmd::Cmd;
use crate::job::{JobKind, JobSpec, Step};
use omarchyvr_config::ai::{Feature, Kind, Provider};
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

/// One line of the one Test (`omarchyvr-ai test --all`: `check <id> <ok|fail|skip> <ms> <detail>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestCheck {
    /// `router`, `action`, `speech`, `agent`.
    pub id: String,
    /// `ok`, `fail` or `skip`.
    pub state: String,
    /// How long the check took.
    pub ms: u64,
    /// Plain words: what was checked and what came back.
    pub detail: String,
}

impl TestCheck {
    /// The name shown in the result line.
    pub fn label(&self) -> &str {
        match self.id.as_str() {
            "router" => "Lara (router)",
            "action" => "Action model",
            "speech" => "Speech recognition",
            "agent" => "Agent model",
            o => o,
        }
    }
}

/// The check lines of `omarchyvr-ai test --all` in a log slice.
pub fn parse_checks<'a>(lines: impl IntoIterator<Item = &'a String>) -> Vec<TestCheck> {
    lines
        .into_iter()
        .filter_map(|l| {
            let mut it = l.trim().splitn(5, ' ');
            if it.next()? != "check" {
                return None;
            }
            let (id, state, ms, detail) = (it.next()?, it.next()?, it.next()?, it.next().unwrap_or(""));
            matches!(state, "ok" | "fail" | "skip").then(|| TestCheck { id: id.into(), state: state.into(), ms: ms.parse().ok().unwrap_or(0), detail: detail.trim().into() })
        })
        .collect()
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

/// `omarchyvr-ai status --json`.
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
    /// The two-stage router ("Lara", port 8081), when it runs.
    pub router: Option<RouterRun>,
    /// Laya (the router's runtime and checkpoint) is downloaded.
    pub laya: bool,
    /// Lara (Laya tuned to the commands) is there; its tuning runs.
    pub lara: bool,
    pub lara_tuning: bool,
    /// Free and total VRAM (MiB).
    pub vram_free_mib: Option<u64>,
    pub vram_total_mib: Option<u64>,
    /// What runs next to the AI ("the Work desk", "a running game", ...).
    pub session: Option<String>,
    /// The speech server (whisper-server, port 8082), when it runs: ready and its video memory (MiB).
    pub asr: Option<AsrRun>,
    /// Why the large speech model cannot start (not built / not downloaded), in plain words.
    pub asr_missing: Option<String>,
}

/// The running speech server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AsrRun {
    pub pid: u32,
    pub ready: bool,
    pub gpu_mib: Option<u64>,
}

/// The running router.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouterRun {
    pub pid: u32,
    /// `laya:english` or a GGUF file.
    pub model: String,
    pub ready: bool,
}

impl LocalAiStatus {
    /// Running on the GPU.
    pub fn on_gpu(&self) -> bool {
        self.running.as_ref().is_some_and(|r| r.mode.starts_with("gpu"))
    }
}

/// Parse the one-line JSON.
pub fn parse_status(text: &str) -> Result<LocalAiStatus, String> {
    let line = text.lines().rev().find(|l| l.trim_start().starts_with('{')).ok_or("omarchyvr-ai status gave no JSON")?;
    let v: Value = serde_json::from_str(line.trim()).map_err(|e| format!("omarchyvr-ai status: {e}"))?;
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
        router: v["router"].as_object().map(|r| RouterRun {
            pid: r.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32,
            model: r.get("model").and_then(Value::as_str).unwrap_or("").to_string(),
            ready: r.get("ready").and_then(Value::as_bool).unwrap_or(false),
        }),
        laya: v["laya"].as_bool().unwrap_or(false),
        lara: v["lara"].as_bool().unwrap_or(false),
        lara_tuning: v["lara_tuning"].as_bool().unwrap_or(false),
        vram_free_mib: v["vram_free_mib"].as_u64(),
        vram_total_mib: v["vram_total_mib"].as_u64(),
        session: s(&v["session"]),
        asr: v["asr"].as_object().map(|r| AsrRun { pid: r.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32, ready: r.get("ready").and_then(Value::as_bool).unwrap_or(false), gpu_mib: r.get("gpu_mib").and_then(Value::as_u64) }),
        asr_missing: s(&v["asr_missing"]),
    })
}

impl LocalAiStatus {
    /// Is a two-stage model id downloaded?
    pub fn has(&self, id: &str) -> bool {
        match id {
            "laya" => self.laya,
            // tuning: Laya serves meanwhile, nothing more to download
            "lara" => self.lara || (self.laya && self.lara_tuning),
            "qwen3.5-4b" => self.models.iter().any(|m| m.id == "model-gpu" && m.installed),
            other => self.models.iter().any(|m| m.id == other && m.installed),
        }
    }
}

/// The demo status: installed, running on the GPU, ready.
pub fn demo_status() -> LocalAiStatus {
    LocalAiStatus {
        installed: true,
        runtime: Some("~/.local/share/omarchyvr/ai/runtime/runtime-vulkan-b11630/llama-b11630/llama-server".into()),
        models: vec![
            LocalModel { id: "model-gpu".into(), file: "Qwen3.5-4B-Q4_K_M.gguf".into(), installed: true },
            LocalModel { id: "model-cpu".into(), file: "Qwen3.5-2B-Q4_K_M.gguf".into(), installed: true },
        ],
        running: Some(Running { pid: 4321, port: 8080, mode: "gpu (all layers)".into(), model: "Qwen3.5-4B-Q4_K_M.gguf".into(), url: LOCAL_BASE.into() }),
        ready: true,
        gpu_mib: Some(3480),
        gpu_budget_mib: Some(4096),
        model_alias: Some(LOCAL_MODEL.into()),
        router: Some(RouterRun { pid: 4400, model: "laya:lara".into(), ready: true }),
        laya: true,
        lara: true,
        lara_tuning: false,
        vram_free_mib: Some(19800),
        vram_total_mib: Some(24564),
        session: Some("the Work desk".into()),
        asr: Some(AsrRun { pid: 4410, ready: true, gpu_mib: Some(790) }),
        asr_missing: None,
    }
}

/// Where `omarchyvr-ai` may be: the workspace build, cargo's bin, ~/.local/bin, a package.
pub fn candidates(release: &Path, home: &Path) -> Vec<PathBuf> {
    vec![release.join("omarchyvr-ai"), home.join(".cargo/bin/omarchyvr-ai"), home.join(".local/bin/omarchyvr-ai"), PathBuf::from("/usr/bin/omarchyvr-ai")]
}

/// `omarchyvr-ai status --json` with a deadline (background only). A status that hangs is left to finish by itself.
fn run_status(bin: &Path) -> Result<LocalAiStatus, String> {
    crate::cmd::allow_start("omarchyvr-ai status")?;
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
    let text = rx.recv_timeout(Duration::from_secs(8)).map_err(|_| "omarchyvr-ai status did not answer within 8 s".to_string())?;
    parse_status(&text)
}

/// The local model's part of the AI tab.
#[derive(Debug, Clone, Default)]
pub struct LocalAiView {
    /// The `omarchyvr-ai` binary found (None: not built or not looked for yet).
    pub bin: Option<PathBuf>,
    /// The last status, or why there is none.
    pub status: Option<Result<LocalAiStatus, String>>,
    /// The last Test: one entry per check (None: not run yet), or why it did not run.
    pub test: Option<Result<Vec<TestCheck>, String>>,
    /// The Test job is running.
    pub testing: bool,
    at: Option<Instant>,
    test_from: Option<usize>,
    /// Run `omarchyvr-ai up` when the pending ai.toml save is done.
    pub follow_after_save: bool,
}

impl AppCore {
    /// Keep the status fresh (every 5 s while the tab is open; in the background).
    pub fn want_localai(&mut self) {
        if self.localai.at.is_some_and(|t| t.elapsed() < Duration::from_secs(5)) || self.task_busy("localai") {
            return;
        }
        self.localai.at = Some(Instant::now());
        if self.demo {
            self.localai.bin = Some(self.paths.bin("omarchyvr-ai"));
            self.localai.status = Some(Ok(demo_status()));
            return;
        }
        let cands = candidates(&self.paths.release, &self.paths.home);
        self.task(Some("localai"), move || {
            let bin = cands.into_iter().find(|p| p.is_file());
            let st = match &bin {
                Some(b) => run_status(b),
                None => Err("omarchyvr-ai is not built yet".into()),
            };
            Box::new(move |c: &mut AppCore| {
                c.localai.bin = bin;
                c.localai.status = Some(st);
            })
        });
    }

    fn localai_spawn(&mut self, args: &[&str], label: &str, own_scope: bool, kind: &str) {
        let Some(bin) = self.localai.bin.clone() else {
            self.message = "omarchyvr-ai is not built yet (cargo build --release -p omarchyvr-ai)".into();
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

    /// Build `omarchyvr-ai` in the workspace (a job; the status finds it afterwards).
    pub fn localai_build(&mut self) {
        let c = Cmd::new("cargo", &["build", "--release", "-p", "omarchyvr-ai"]).cwd(&self.paths.rust);
        if self.spawn(JobSpec { kind: JobKind::Other(KIND.into()), label: "Build omarchyvr-ai".into(), steps: vec![Step::Run(c)] }) {
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

    /// Stop the server (and the two-stage router with it).
    pub fn localai_stop(&mut self) {
        self.localai_spawn(&["down"], "Stop the local model", false, KIND);
    }

    /// Download what the two-stage setting needs (the router "Lara" and the action model of ai.toml).
    pub fn localai_install_two_stage(&mut self) {
        let t = self.ai.config.two_stage();
        let mib: u32 = [&t.router, &t.action].iter().filter_map(|id| omarchyvr_config::ai::local_model(id)).map(|m| m.download_mib).sum();
        self.localai_spawn(&["install", "--two-stage", "--router", &t.router, "--action", &t.action], &format!("Download the two-stage AI ({:.1} GB at most)", mib as f64 / 1024.0), false, KIND);
    }

    /// Start what ai.toml asks for (simple or two stages, fitted to the free VRAM).
    pub fn localai_up(&mut self) {
        self.localai_spawn(&["up"], "Start the local AI", true, KIND);
    }

    /// After a change of the local setting: the running servers follow it (`omarchyvr-ai up`) once ai.toml is written;
    /// nothing starts when no local server runs (the next Play / Work start brings it up).
    pub fn localai_follow(&mut self) {
        if self.demo || !self.task_busy("ai-save") {
            self.localai_follow_now();
        } else {
            self.localai.follow_after_save = true;
        }
    }

    /// The same, now.
    pub fn localai_follow_now(&mut self) {
        let runs = self.localai.status.as_ref().and_then(|s| s.as_ref().ok()).is_some_and(|s| s.running.is_some());
        if runs && self.job.is_none() {
            self.localai_up();
        }
    }

    /// The one Test: every check in order (Lara, the action model, speech, the agent model when it is on), timed.
    pub fn localai_test(&mut self) {
        self.localai.test = None;
        self.localai_spawn(&["test", "--all"], "Test the local AI", false, TEST_KIND);
        self.localai.testing = self.job.is_some();
    }

    /// A local-model job ended (the status is asked again; a test's lines are kept).
    pub(crate) fn localai_done(&mut self, kind: &str, ok: bool) {
        self.localai.at = None;
        if kind == TEST_KIND {
            self.localai.testing = false;
            let from = self.localai.test_from.take().unwrap_or(0).min(self.log.len());
            let checks = parse_checks(&self.log[from..]);
            self.localai.test = Some(if checks.is_empty() {
                Err(if ok { "The test printed no result (see the output below).".into() } else { "The test could not run (see the output below).".into() })
            } else {
                Ok(checks)
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
        // two stages: the router, Laya / Lara, the free VRAM and the session
        let two = r#"{"installed":true,"runtime":"/r","models":[{"id":"model-gpu","file":"Qwen3.5-4B-Q4_K_M.gguf","installed":true},{"id":"qwen3.5-9b","file":"Qwen3.5-9B-IQ4_XS.gguf","installed":false}],"running":null,"ready":false,"gpu_mib":null,"gpu_budget_mib":4096,"model_alias":"pimax-local","router":{"pid":9,"port":8081,"model":"laya:lara","ready":true},"laya":true,"lara":true,"vram_free_mib":19800,"vram_total_mib":24564,"session":"the Work desk"}"#;
        let s = parse_status(two).unwrap();
        assert_eq!(s.router, Some(RouterRun { pid: 9, model: "laya:lara".into(), ready: true }));
        assert!(s.has("lara") && s.has("laya") && s.has("qwen3.5-4b") && !s.has("qwen3.5-9b"));
        assert_eq!((s.vram_free_mib, s.vram_total_mib, s.session.as_deref()), (Some(19800), Some(24564), Some("the Work desk")));
    }

    #[test]
    fn the_test_lines() {
        let log: Vec<String> = ["starting", "check router ok 31 Lara sorted a test command: system", "check action ok 190 Qwen3.5-4B-Q4_K_M (gpu); 3.4 GB", "check speech skip 0 large model off", "check agent fail 12 x: refused", "check bogus maybe 1 x", "check action"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let c = parse_checks(&log);
        assert_eq!(c.len(), 4);
        assert_eq!((c[0].id.as_str(), c[0].state.as_str(), c[0].ms), ("router", "ok", 31));
        assert_eq!(c[0].detail, "Lara sorted a test command: system");
        assert_eq!((c[2].state.as_str(), c[3].label()), ("skip", "Agent model"));
    }

    #[test]
    fn use_local_model_adds_the_provider_once_and_picks_it_for_voice() {
        let d = std::env::temp_dir().join(format!("pimax-localai-{}", std::process::id()));
        let mut a = AppCore::demo(true, omarchyvr_config::Store::new(d.join("a"), d.join("b")));
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
