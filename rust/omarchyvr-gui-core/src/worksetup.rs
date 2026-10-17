//! The Work first run (docs/WORK-FIRST-RUN.md). The first time Work is pressed, the app asks two questions before VR
//! starts (the user is still at the desktop, not in the headset), then starts Work by itself:
//!
//! 1. **Voice assistant**: install Lara (the command router on this PC: ~2.7 GB, Apache-2.0, processor only) or not, and
//!    the "brain" that answers what the built-in commands do not know: Claude (the Claude Code program on this PC with
//!    its own sign-in, or the user's own Anthropic API key), the local model (Qwen3.5-4B, ~2.6 GB, on the processor; the
//!    graphics card only when chosen), any other compatible API (base URL + key + model, with a Test), or none.
//! 2. **Speech to text**: on the processor (Parakeet, the default), on the graphics card (whisper, opt-in), or off.
//!
//! Downloads go through `omarchyvr-ai get` (the same pinned files, SHA-256 checks and reuse of what is on disk as
//! `omarchyvr-ai install`), with progress per item and Cancel (SIGTERM to that one child, by its pid; the part already
//! downloaded stays for the next try). The answers go where the stack reads them: ai.toml (`omarchyvr_config::ai`, written
//! atomically with mode 600; a key only there, never in a log or in this module's Debug output) and the Work switches
//! (voice on or off). `work-setup.toml` records that the questions were answered, so later Work presses start at once;
//! the AI page reopens the questions. Play never shows them.
use crate::app::{AppCore, Page};
use crate::cmd::Cmd;
use omarchyvr_config::ai::{self, AiConfig, Asr, Feature, Kind, Provider, Secret, VoiceAi, VoiceLocal};
use omarchyvr_config::claude_cli::ClaudeCliStatus;
pub use omarchyvr_config::work_setup::{Brain, Speech, WorkSetup};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The wizard's pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Step {
    /// Lara and the brain.
    #[default]
    Assistant,
    /// Speech to text.
    Speech,
    /// The downloads (only when something is missing).
    Get,
}

/// The answers being chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choices {
    pub lara: bool,
    pub brain: Brain,
    /// The local model may use the graphics card (default: the processor).
    pub local_gpu: bool,
    pub speech: Speech,
    /// Claude with a key: the model (an id of `ai::CLAUDE_MODELS`, Haiku by default).
    pub claude_model: String,
    /// Another compatible API: its base URL and model.
    pub base_url: String,
    pub model: String,
}

impl Default for Choices {
    fn default() -> Self {
        Choices {
            lara: false,
            brain: Brain::ClaudeCli,
            local_gpu: false,
            speech: Speech::Cpu,
            claude_model: ai::CLAUDE_MODELS[0].0.to_string(),
            base_url: String::new(),
            model: String::new(),
        }
    }
}

/// What is on this PC already (looked at in the background when the wizard opens).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OnDisk {
    /// llama.cpp's runtime (`omarchyvr-ai`).
    pub runtime: bool,
    /// Lara's base (Laya and its Python environment).
    pub laya: bool,
    /// Lara tuned to the voice commands.
    pub lara: bool,
    /// Qwen3.5-4B.
    pub qwen4b: bool,
    /// The speech model on the processor (Parakeet).
    pub parakeet: bool,
    /// Speech on the graphics card: whisper-server is built and its model is there.
    pub whisper: bool,
    /// `omarchyvr-ai` (the downloads and the local AI).
    pub ai_bin: Option<PathBuf>,
    /// The Claude Code program (`claude --version`).
    pub claude: Option<ClaudeCliStatus>,
}

/// The ids `omarchyvr-ai get` knows, with their download size and label.
pub const ITEMS: [(&str, u64, &str); 3] = [
    ("runtime", 30_416_480, "llama.cpp runtime (MIT)"),
    ("lara", omarchyvr_ai::twostage::LAYA_DOWNLOAD_MIB << 20, "Lara, the voice command router (Apache-2.0)"),
    ("qwen3.5-4b", 2_740_937_888, "Qwen3.5-4B, the local model (Apache-2.0)"),
];

/// Lara is used with this choice (the brain acts after Lara sorts).
pub fn lara_used(c: &Choices) -> bool {
    c.lara && c.brain.takes_lara()
}

/// What must be downloaded for these answers (ids of [`ITEMS`], in download order); empty = nothing.
pub fn downloads(c: &Choices, d: &OnDisk) -> Vec<&'static str> {
    let mut v = Vec::new();
    if c.brain == Brain::Local && !d.runtime {
        v.push("runtime");
    }
    if lara_used(c) && !d.laya {
        v.push("lara");
    }
    if c.brain == Brain::Local && !d.qwen4b {
        v.push("qwen3.5-4b");
    }
    v
}

/// The bytes of a list of downloads.
pub fn bytes_of(ids: &[&str]) -> u64 {
    ids.iter().filter_map(|id| ITEMS.iter().find(|x| x.0 == *id)).map(|x| x.1).sum()
}

/// "2.6 GB", or "29 MB" under a gigabyte.
pub fn gb(bytes: u64) -> String {
    if bytes < 1_073_741_824 {
        format!("{} MB", (bytes + 524_288) / 1_048_576)
    } else {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    }
}

/// A download error without the retry hint of the command line (the wizard has its own Try again button).
pub fn plain_download_error(e: &str) -> String {
    let mut s = e.to_string();
    for tail in ["; press Download again to retry (it resumes)", "; run install again to resume", "; check the internet connection and try again (it goes on where it stopped)"] {
        if let Some(i) = s.find(tail) {
            s.truncate(i);
        }
    }
    s
}

/// Why the first page (the voice assistant) cannot go on yet, in plain words (None: it can). `key` = the key field;
/// `saved_key` = ai.toml already holds a key for that brain.
pub fn problem_assistant(c: &Choices, key: &str, saved_key: bool, d: &OnDisk) -> Option<String> {
    let url_ok = |u: &str| (u.starts_with("https://") || u.starts_with("http://")) && !u.contains(['"', '\\', ' ']);
    match c.brain {
        Brain::ClaudeKey if key.trim().is_empty() && !saved_key => Some("Paste your Anthropic API key (it starts with sk-ant-).".into()),
        Brain::Compatible if !url_ok(c.base_url.trim()) => Some("Enter the API's base URL (https://..., up to /v1).".into()),
        Brain::Compatible if c.model.trim().is_empty() => Some("Enter the model name the API offers.".into()),
        Brain::Local if d.ai_bin.is_none() && !(d.runtime && d.qwen4b) => Some("omarchyvr-ai is missing on this PC, so the local model cannot be downloaded (run the installer again).".into()),
        _ if lara_used(c) && d.ai_bin.is_none() && !d.laya => Some("omarchyvr-ai is missing on this PC, so Lara cannot be downloaded (run the installer again).".into()),
        _ => None,
    }
}

/// Why the second page (speech to text) cannot be saved.
pub fn problem_speech(c: &Choices, d: &OnDisk) -> Option<String> {
    if c.speech != Speech::Gpu {
        return None;
    }
    if !d.whisper {
        return Some("Speech on the graphics card is not installed on this PC: choose the processor.".into());
    }
    (!gpu_speech_possible(c)).then(|| "Speech on the graphics card works only with the model on this PC (it runs next to it): choose the processor.".into())
}

/// The graphics-card speech server runs only next to the local models (`omarchyvr-ai up` starts it with them;
/// omarchyvr-voice uses it only when they run): with another brain it would silently be the processor.
pub fn gpu_speech_possible(c: &Choices) -> bool {
    c.brain == Brain::Local
}

/// Why the answers cannot be saved yet (either page).
pub fn problem(c: &Choices, key: &str, saved_key: bool, d: &OnDisk) -> Option<String> {
    problem_assistant(c, key, saved_key, d).or_else(|| problem_speech(c, d))
}

/// Write the answers into ai.toml's model `cfg`: the brain (`[use] voice`, the provider and its key), Lara, the local
/// models and the speech engine. Other providers, their keys and every key or section this version does not know stay
/// as they are. `key`: a key just typed (None: keep the one in the file).
pub fn apply(c: &Choices, key: Option<Secret>, cfg: &mut AiConfig) -> Result<(), String> {
    cfg.asr = if c.speech == Speech::Gpu { Asr::Gpu } else { Asr::Cpu };
    cfg.voice_ai = match (c.brain, cfg.voice_ai) {
        (Brain::None, _) => VoiceAi::Off,
        (_, VoiceAi::Off) => VoiceAi::Fallback,
        (_, v) => v,
    };
    let free_name = |cfg: &AiConfig, base: &str, kind: Kind| -> String {
        // the provider of that kind with that name, else a free name
        if cfg.providers.iter().any(|p| p.name == base && p.kind == kind) || cfg.provider(base).is_none() {
            return base.to_string();
        }
        (2..).map(|i| format!("{base}-{i}")).find(|n| cfg.provider(n).is_none()).unwrap_or_else(|| base.to_string())
    };
    let put = |cfg: &mut AiConfig, p: Provider| match cfg.providers.iter().position(|x| x.name == p.name) {
        Some(i) => cfg.providers[i] = p,
        None => cfg.providers.push(p),
    };
    match c.brain {
        Brain::ClaudeCli => {
            cfg.voice_local_models = Some(false);
            cfg.voice_lara = Some(c.lara);
            let name = cfg.claude_cli_provider().name.clone();
            cfg.uses.insert(Feature::Voice, name);
        }
        Brain::ClaudeKey => {
            let name = free_name(cfg, "claude", Kind::Anthropic);
            let mut p = cfg.providers.iter().find(|p| p.name == name).cloned().unwrap_or_else(|| Provider::new(&name, Kind::Anthropic));
            if !ai::CLAUDE_MODELS.iter().any(|(id, _)| *id == c.claude_model) {
                return Err(format!("{}: not a Claude model", c.claude_model));
            }
            p.model = c.claude_model.clone();
            match key {
                Some(k) if !k.is_empty() => {
                    p.api_key = Some(k);
                    p.api_key_env = None;
                }
                _ if p.api_key.is_some() || p.api_key_env.is_some() => {}
                _ => return Err("Paste your Anthropic API key first.".into()),
            }
            put(cfg, p);
            cfg.uses.insert(Feature::Voice, name);
            cfg.voice_local_models = Some(false);
            cfg.voice_lara = Some(c.lara);
        }
        Brain::Compatible => {
            let name = free_name(cfg, "api", Kind::OpenAiCompatible);
            let mut p = cfg.providers.iter().find(|p| p.name == name).cloned().unwrap_or_else(|| Provider::new(&name, Kind::OpenAiCompatible));
            p.base_url = Some(c.base_url.trim().trim_end_matches('/').to_string());
            p.model = c.model.trim().to_string();
            if let Some(k) = key.filter(|k| !k.is_empty()) {
                p.api_key = Some(k);
                p.api_key_env = None;
            }
            put(cfg, p);
            cfg.uses.insert(Feature::Voice, name);
            cfg.voice_local_models = Some(false);
            cfg.voice_lara = Some(c.lara);
        }
        Brain::Local => {
            cfg.set_local_models(true, false)?;
            if c.lara {
                cfg.voice_local = VoiceLocal::TwoStage;
                cfg.set_preset(if c.local_gpu { ai::Preset::Medium } else { ai::Preset::Low });
            } else {
                // one model, no router: on the processor unless the graphics card was chosen
                cfg.voice_local = VoiceLocal::Simple;
                let t = cfg.local_two_stage.get_or_insert_with(ai::LocalTwoStage::default);
                t.gpu_layers = if c.local_gpu { None } else { Some(0) };
                t.preset = t.matching();
            }
            cfg.voice_lara = None;
        }
        Brain::None => {
            cfg.voice_local_models = Some(false);
            cfg.voice_lara = None;
        }
    }
    Ok(())
}

/// ai.toml exists and already configures a brain (a provider, a default or a `[use]` entry for voice, or voice turned
/// off on purpose): the user set the AI up before, so the Work questions count as answered (user, 2026-10-17). They
/// stay reachable from AI > "Work questions".
pub fn ai_configured(path: &std::path::Path) -> bool {
    if !path.is_file() {
        return false;
    }
    match AiConfig::load_from(path) {
        Ok(cfg) => !cfg.providers.is_empty() || cfg.default.is_some() || cfg.uses.contains_key(&Feature::Voice) || cfg.voice_ai == VoiceAi::Off,
        Err(_) => false,
    }
}

/// The answers as ai.toml says them now (the wizard opens with the current setup).
pub fn choices_from(cfg: &AiConfig, rec: &WorkSetup) -> Choices {
    let mut c = Choices::default();
    let voice = cfg.provider_for(Feature::Voice);
    c.brain = match voice {
        _ if cfg.voice_ai == VoiceAi::Off => Brain::None,
        Some(p) if p.kind == Kind::ClaudeCli => Brain::ClaudeCli,
        Some(p) if p.kind == Kind::Anthropic => Brain::ClaudeKey,
        Some(p) if p.is_remote() => Brain::Compatible,
        Some(_) if cfg.local_models_on() => Brain::Local,
        _ => rec.brain.unwrap_or(Brain::ClaudeCli),
    };
    if let Some(p) = voice {
        match c.brain {
            Brain::ClaudeKey => c.claude_model = Some(p.model.clone()).filter(|m| ai::CLAUDE_MODELS.iter().any(|(id, _)| id == m)).unwrap_or(c.claude_model),
            Brain::Compatible => {
                c.base_url = p.base_url.clone().unwrap_or_default();
                c.model = p.model.clone();
            }
            _ => {}
        }
    }
    c.lara = match c.brain {
        Brain::Local => cfg.voice_local == VoiceLocal::TwoStage,
        Brain::ClaudeCli | Brain::ClaudeKey | Brain::Compatible => cfg.voice_lara == Some(true),
        _ => rec.lara,
    };
    c.local_gpu = if c.brain == Brain::Local { cfg.two_stage().gpu_layers != Some(0) } else { rec.local_gpu };
    // speech off is Work's voice switch (the app's state), set by the caller; the graphics card only next to the local
    // model (anything else runs on the processor, whatever ai.toml says)
    c.speech = if cfg.asr == Asr::Gpu && gpu_speech_possible(&c) { Speech::Gpu } else { Speech::Cpu };
    c
}

/// One download of the wizard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub label: String,
    /// Expected bytes (0: already on this PC).
    pub total: u64,
    pub done: u64,
    pub state: ItemState,
}

/// Where one download is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemState {
    Waiting,
    Running,
    Done,
    /// Already on this PC (checked).
    Here,
    Failed(String),
    Cancelled,
}

/// The running `omarchyvr-ai get` (shared with its reader thread).
#[derive(Debug, Default)]
pub struct GetState {
    pub items: Vec<Item>,
    /// The newest plain-words note of the tool ("resuming at 1200 MiB of 2614 MiB", "checking SHA-256").
    pub note: String,
    /// The child's pid while it runs (Cancel signals it).
    pub pid: Option<u32>,
    /// Ok when everything is here; the plain-words error, or "cancelled".
    pub finished: Option<Result<(), String>>,
}

impl GetState {
    /// Fraction done over every item (by bytes).
    pub fn fraction(&self) -> f32 {
        let total: u64 = self.items.iter().map(|i| i.total).sum();
        if total == 0 {
            return if self.finished.is_some() { 1.0 } else { 0.0 };
        }
        let done: u64 = self.items.iter().map(|i| if matches!(i.state, ItemState::Done | ItemState::Here) { i.total } else { i.done.min(i.total) }).sum();
        done as f32 / total as f32
    }
}

/// Apply one output line of `omarchyvr-ai get` (`get plan|progress|note|done|error|cancelled|all done ...`).
pub fn apply_line(st: &mut GetState, line: &str) {
    let mut w = line.splitn(4, ' ');
    if w.next() != Some("get") {
        return;
    }
    let kind = w.next().unwrap_or("");
    let id = w.next().unwrap_or("").to_string();
    let rest = w.next().unwrap_or("").to_string();
    let item = |st: &mut GetState, id: &str| st.items.iter().position(|i| i.id == id);
    match kind {
        "plan" => {
            let (n, label) = rest.split_once(' ').unwrap_or((rest.as_str(), ""));
            let total = n.parse().unwrap_or(0);
            let it = Item { id: id.clone(), label: label.to_string(), total, done: 0, state: if total == 0 { ItemState::Here } else { ItemState::Waiting } };
            match item(st, &id) {
                Some(i) => st.items[i] = it,
                None => st.items.push(it),
            }
        }
        "progress" => {
            if let Some(i) = item(st, &id) {
                st.items[i].done = rest.trim().parse().unwrap_or(st.items[i].done);
                st.items[i].state = ItemState::Running;
            }
        }
        "note" => {
            st.note = format!("{id} {rest}").trim().to_string();
            // a note while nothing runs yet: the tool started on the next item waiting
            if !st.items.iter().any(|i| i.state == ItemState::Running) {
                if let Some(it) = st.items.iter_mut().find(|i| i.state == ItemState::Waiting) {
                    it.state = ItemState::Running;
                }
            }
        }
        "done" => {
            if let Some(i) = item(st, &id) {
                if st.items[i].state != ItemState::Here {
                    st.items[i].state = ItemState::Done;
                    st.items[i].done = st.items[i].total;
                }
            }
        }
        "error" => {
            if let Some(i) = item(st, &id) {
                st.items[i].state = ItemState::Failed(rest.clone());
            }
            st.finished = Some(Err(rest));
        }
        "cancelled" => {
            if let Some(i) = item(st, &id) {
                st.items[i].state = ItemState::Cancelled;
            }
            st.finished = Some(Err(omarchyvr_ai::CANCELLED.into()));
        }
        "all" if id == "done" => {
            if st.finished.is_none() {
                st.finished = Some(Ok(()));
            }
        }
        _ => {}
    }
}

/// The wizard's state.
#[derive(Default)]
pub struct WorkSetupView {
    /// The record (`work-setup.toml`): `done` = the questions were answered.
    pub record: WorkSetup,
    pub step: Step,
    pub choices: Choices,
    /// The key field (Claude with a key, another API). Wiped when the answers are saved or the wizard closes; never
    /// printed (this type's Debug leaves it out).
    pub key: String,
    /// Show the key while typing.
    pub show_key: bool,
    /// What is on this PC (None: still looking).
    pub on_disk: Option<OnDisk>,
    /// The connection test of a typed key / API: the answer or the plain error (None: not run).
    pub test: Option<Result<String, String>>,
    pub testing: bool,
    /// The downloads (while and after they run).
    pub get: Option<Arc<Mutex<GetState>>>,
    /// The last plain-words problem (a save that failed, a cancelled download).
    pub error: Option<String>,
    /// Start Work when the answers are saved (the first Work press).
    pub start_after: bool,
    /// The page to go back to when the wizard closes.
    pub back_to: Option<Page>,
}

impl std::fmt::Debug for WorkSetupView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkSetupView")
            .field("record", &self.record)
            .field("step", &self.step)
            .field("choices", &self.choices)
            .field("key", &if self.key.is_empty() { "<empty>" } else { "<hidden>" })
            .field("on_disk", &self.on_disk)
            .field("test", &self.test)
            .field("error", &self.error)
            .field("start_after", &self.start_after)
            .finish()
    }
}

impl WorkSetupView {
    /// Wipe the typed key (overwrite, then clear).
    pub fn wipe_key(&mut self) {
        let n = self.key.len();
        self.key.replace_range(.., &"\0".repeat(n));
        self.key.clear();
        self.show_key = false;
    }

    /// The downloads these answers need now (empty while the look at the disk runs).
    pub fn needed(&self) -> Vec<&'static str> {
        self.on_disk.as_ref().map(|d| downloads(&self.choices, d)).unwrap_or_default()
    }

    /// The downloads run.
    pub fn getting(&self) -> bool {
        self.get.as_ref().is_some_and(|g| g.lock().map(|s| s.finished.is_none()).unwrap_or(false))
    }
}

/// The speech model on the processor (Parakeet) is on disk.
pub fn parakeet_present(model_dir: &Path) -> bool {
    ["encoder.int8.onnx", "decoder.int8.onnx", "joiner.int8.onnx", "tokens.txt"].iter().all(|f| model_dir.join(f).is_file())
}

/// Look at the disk (background only): the AI's downloads, the speech models, `omarchyvr-ai`, the Claude CLI.
pub fn scan(home: &Path, voice_model: &Path, ai_bins: &[PathBuf], claude_cmd: &str, env: &dyn Fn(&str) -> Option<String>) -> OnDisk {
    let dirs = omarchyvr_ai::Dirs::from_env(env);
    let whisper = omarchyvr_ai::twostage::asr_missing(&omarchyvr_config::paths::whisper_server(home), &home.join("models/whisper/ggml-large-v3-turbo-q5_0.bin")).is_none();
    OnDisk {
        runtime: omarchyvr_ai::installed_server(&dirs).is_some(),
        laya: omarchyvr_ai::twostage::laya_installed(&dirs),
        lara: omarchyvr_ai::twostage::lara_installed(&dirs),
        qwen4b: omarchyvr_ai::twostage::model_installed(&dirs, "qwen3.5-4b"),
        parakeet: parakeet_present(voice_model),
        whisper,
        ai_bin: ai_bins.iter().find(|p| p.is_file()).cloned(),
        claude: Some(omarchyvr_config::claude_cli::claude_cli_status(claude_cmd)),
    }
}

/// What demo mode (screenshots, tests) finds on disk: the speech model only, nothing of the AI yet.
pub fn demo_on_disk() -> OnDisk {
    OnDisk {
        parakeet: true,
        ai_bin: Some(PathBuf::from("/home/u/.local/bin/omarchyvr-ai")),
        claude: Some(ClaudeCliStatus::Found { path: "/usr/bin/claude".into(), version: "2.1.0 (Claude Code)".into() }),
        ..OnDisk::default()
    }
}

/// Start `omarchyvr-ai get IDS...` and follow it on a thread of its own: every line goes into `st`, `wake` repaints.
pub fn run_get(bin: &Path, ids: &[&str], st: Arc<Mutex<GetState>>, wake: Arc<dyn Fn() + Send + Sync>) -> Result<(), String> {
    use std::io::BufRead;
    let mut args = vec!["get".to_string()];
    args.extend(ids.iter().map(|s| s.to_string()));
    let c = Cmd::with_args(bin.display().to_string(), args.clone());
    crate::safety::check(&c).map_err(|e| format!("refused: {e}"))?;
    crate::cmd::allow_start("omarchyvr-ai get")?;
    let mut child = std::process::Command::new(bin)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("{}: {e}", bin.display()))?;
    if let Ok(mut s) = st.lock() {
        s.pid = Some(child.id());
        s.finished = None;
    }
    let out = child.stdout.take();
    let err = child.stderr.take();
    std::thread::Builder::new()
        .name("work-setup-get".into())
        .spawn(move || {
            // stderr: the last line names the problem when the tool fails before saying `get error`
            let last_err = Arc::new(Mutex::new(String::new()));
            let le = last_err.clone();
            let e_thread = err.map(|e| {
                std::thread::spawn(move || {
                    for l in std::io::BufReader::new(e).lines().map_while(Result::ok) {
                        if !l.trim().is_empty() {
                            if let Ok(mut x) = le.lock() {
                                *x = l.trim_start_matches("omarchyvr-ai: ").to_string();
                            }
                        }
                    }
                })
            });
            if let Some(o) = out {
                for l in std::io::BufReader::new(o).lines().map_while(Result::ok) {
                    if let Ok(mut s) = st.lock() {
                        apply_line(&mut s, &l);
                    }
                    wake();
                }
            }
            if let Some(t) = e_thread {
                let _ = t.join();
            }
            let status = child.wait();
            if let Ok(mut s) = st.lock() {
                s.pid = None;
                if s.finished.is_none() {
                    s.finished = Some(match status {
                        Ok(st) if st.success() => Ok(()),
                        Ok(st) if st.code() == Some(130) => Err(omarchyvr_ai::CANCELLED.into()),
                        Ok(st) => Err(last_err.lock().map(|x| x.clone()).ok().filter(|x| !x.is_empty()).unwrap_or_else(|| format!("the download stopped ({st})"))),
                        Err(e) => Err(e.to_string()),
                    });
                }
            }
            wake();
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

impl AppCore {
    /// The answers were given (the next Work starts at once).
    pub fn work_setup_done(&self) -> bool {
        self.work_setup.record.done
    }

    /// `work-setup.toml`.
    pub fn work_setup_path(&self) -> PathBuf {
        WorkSetup::path(&self.paths.config)
    }

    /// Open the questions. `start_after`: Work starts when they are answered (the first Work press); false = from the
    /// AI page (change the answers).
    pub fn open_work_setup(&mut self, start_after: bool) {
        self.want_ai();
        let ws = &mut self.work_setup;
        if ws.get.is_none() || !ws.getting() {
            ws.step = Step::Assistant;
            ws.get = None;
        }
        ws.error = None;
        ws.test = None;
        ws.start_after = start_after;
        if self.page != Page::WorkSetup {
            ws.back_to = Some(self.page);
        }
        if self.ai.loaded && self.ai.error.is_none() {
            self.work_setup.choices = choices_from(&self.ai.config, &self.work_setup.record);
        }
        if !self.toggles.voice {
            self.work_setup.choices.speech = Speech::Off;
        }
        self.page = Page::WorkSetup;
        self.ws_look();
    }

    /// Look at the disk again (background).
    pub fn ws_look(&mut self) {
        if self.demo {
            self.work_setup.on_disk.get_or_insert_with(demo_on_disk);
            return;
        }
        let home = self.paths.home.clone();
        let voice = self.paths.voice_model.clone();
        let bins = crate::localai::candidates(&self.paths.release, &self.paths.home);
        let cmd = self.claude_command();
        let env = self.env.clone();
        self.task(Some("work-setup-look"), move || {
            let d = scan(&home, &voice, &bins, &cmd, &|k| env(k));
            Box::new(move |c: &mut AppCore| c.work_setup.on_disk = Some(d))
        });
    }

    /// The choices of ai.toml arrived after the wizard opened: take them over once (nothing typed is lost).
    pub(crate) fn ws_sync_from_ai(&mut self) {
        if self.page == Page::WorkSetup && self.work_setup.step == Step::Assistant && self.ai.loaded && self.ai.error.is_none() && !self.work_setup.record.done {
            let c = choices_from(&self.ai.config, &self.work_setup.record);
            if c != self.work_setup.choices && self.work_setup.key.is_empty() && self.work_setup.choices == Choices::default() {
                self.work_setup.choices = c;
            }
        }
    }

    /// A key for the chosen brain is already in ai.toml (or comes from the environment).
    pub fn ws_saved_key(&self) -> bool {
        let kind = match self.work_setup.choices.brain {
            Brain::ClaudeKey => Kind::Anthropic,
            Brain::Compatible => Kind::OpenAiCompatible,
            _ => return false,
        };
        self.ai.config.providers.iter().any(|p| p.kind == kind && (p.api_key.is_some() || p.api_key_env.as_deref().and_then(|e| (self.env)(e)).is_some()))
    }

    /// Why Next / Finish cannot go on yet (plain words).
    pub fn ws_problem(&self) -> Option<String> {
        let d = self.work_setup.on_disk.clone().unwrap_or_default();
        problem(&self.work_setup.choices, &self.work_setup.key, self.ws_saved_key(), &d)
    }

    /// Next: the speech question (the speech answer itself is checked on its own page).
    pub fn ws_next(&mut self) {
        let d = self.work_setup.on_disk.clone().unwrap_or_default();
        if let Some(p) = problem_assistant(&self.work_setup.choices, &self.work_setup.key, self.ws_saved_key(), &d) {
            self.work_setup.error = Some(p);
            return;
        }
        self.work_setup.error = None;
        // another brain than the local model: its speech server cannot run, the processor is the answer
        if self.work_setup.choices.speech == Speech::Gpu && !gpu_speech_possible(&self.work_setup.choices) {
            self.work_setup.choices.speech = Speech::Cpu;
        }
        self.work_setup.step = Step::Speech;
    }

    /// Back one page (the downloads: only once they ended).
    pub fn ws_back(&mut self) {
        self.work_setup.error = None;
        self.work_setup.step = match self.work_setup.step {
            Step::Assistant | Step::Speech => Step::Assistant,
            Step::Get if self.work_setup.getting() => Step::Get,
            Step::Get => Step::Speech,
        };
    }

    /// Close without saving (Work does not start; the questions come again next time).
    pub fn ws_close(&mut self) {
        if self.work_setup.getting() {
            self.ws_cancel();
        }
        self.work_setup.wipe_key();
        self.work_setup.error = None;
        self.page = self.work_setup.back_to.take().unwrap_or(Page::Desk);
    }

    /// Finish: download what is missing (with progress), then save and (first run) start Work.
    pub fn ws_finish(&mut self) {
        if let Some(p) = self.ws_problem() {
            self.work_setup.error = Some(p);
            return;
        }
        let ids = self.work_setup.needed();
        if ids.is_empty() {
            self.ws_save();
            return;
        }
        self.work_setup.error = None;
        self.work_setup.step = Step::Get;
        let st = Arc::new(Mutex::new(GetState::default()));
        self.work_setup.get = Some(st.clone());
        if self.demo {
            // demo: the plan and a download half way, nothing is fetched
            if let Ok(mut s) = st.lock() {
                for id in &ids {
                    if let Some(x) = ITEMS.iter().find(|x| x.0 == *id) {
                        apply_line(&mut s, &format!("get plan {} {} {}", x.0, x.1, x.2));
                    }
                }
                apply_line(&mut s, &format!("get progress {} {}", ids[0], bytes_of(&ids[..1]) / 2));
                apply_line(&mut s, "get note demo mode: nothing is downloaded");
            }
            return;
        }
        let Some(bin) = self.work_setup.on_disk.as_ref().and_then(|d| d.ai_bin.clone()) else {
            self.work_setup.error = Some("omarchyvr-ai is missing on this PC (run the installer again).".into());
            return;
        };
        if let Err(e) = run_get(&bin, &ids, st, self.wake.clone()) {
            self.work_setup.error = Some(e);
        }
    }

    /// Cancel the downloads: SIGTERM to the `omarchyvr-ai get` this wizard started (by its pid); the part already
    /// downloaded stays for the next try.
    pub fn ws_cancel(&mut self) {
        let Some(st) = self.work_setup.get.clone() else { return };
        let pid = st.lock().ok().and_then(|s| s.pid);
        if self.demo {
            if let Ok(mut s) = st.lock() {
                apply_line(&mut s, "get cancelled lara");
                s.finished = Some(Err(omarchyvr_ai::CANCELLED.into()));
            }
            return;
        }
        if let Some(pid) = pid.filter(|p| *p > 1) {
            // SAFETY: kill(2) on our own child (its reader thread has not reaped it yet, so the pid is still ours).
            unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        }
    }

    /// Follow the downloads (from [`AppCore::poll`]): when they end, save (all here) or say why not.
    pub(crate) fn ws_poll(&mut self) -> bool {
        let Some(st) = self.work_setup.get.clone() else { return false };
        let fin = st.lock().ok().and_then(|s| s.finished.clone());
        match fin {
            Some(Ok(())) if self.work_setup.step == Step::Get && !self.demo => {
                self.ws_look();
                self.ws_save();
                true
            }
            Some(Err(e)) if self.work_setup.error.is_none() && self.work_setup.step == Step::Get => {
                self.work_setup.error = Some(if e == omarchyvr_ai::CANCELLED {
                    "Cancelled. What was already downloaded is kept: Download goes on from there.".into()
                } else {
                    format!("The download stopped: {}. Check the internet connection, then Try again: it goes on where it stopped.", plain_download_error(&e))
                });
                true
            }
            _ => false,
        }
    }

    /// Save the answers: ai.toml (mode 600, the key only there), the voice switch of Work, the record; then start Work
    /// when this was the first Work press.
    pub fn ws_save(&mut self) {
        if !self.ai.loaded {
            self.work_setup.error = Some("The AI settings are still being read: try again in a moment.".into());
            self.want_ai();
            return;
        }
        if let Some(e) = &self.ai.error {
            self.work_setup.error = Some(format!("ai.toml could not be read ({e}): fix it on the AI page first."));
            return;
        }
        let c = self.work_setup.choices.clone();
        let key = Some(Secret::new(&self.work_setup.key)).filter(|k| !k.is_empty());
        let mut cfg = self.ai.config.clone();
        if let Err(e) = apply(&c, key, &mut cfg) {
            self.work_setup.error = Some(e);
            return;
        }
        self.work_setup.wipe_key();
        self.ai.config = cfg;
        self.ai_save();
        // Work's own switch: voice (commands and subtitles) unless speech is off
        self.toggles.voice = c.speech != Speech::Off;
        self.save_state();
        let rec = WorkSetup { done: true, at: self.machine.now_unix(), lara: lara_used(&c), brain: Some(c.brain), local_gpu: c.local_gpu, speech: Some(c.speech) };
        self.work_setup.record = rec.clone();
        if !self.demo {
            let path = self.work_setup_path();
            self.task(None, move || {
                let r = rec.save(&path);
                Box::new(move |core: &mut AppCore| {
                    if let Err(e) = r {
                        core.message = format!("The Work setup answers were saved, but not the record: {e}");
                    }
                })
            });
        }
        self.work_setup.error = None;
        self.work_setup.get = None;
        self.work_setup.step = Step::Assistant;
        let start = std::mem::take(&mut self.work_setup.start_after);
        self.page = self.work_setup.back_to.take().filter(|_| !start).unwrap_or(Page::Desk);
        self.message = "Work setup saved".into();
        if start {
            self.work();
        }
    }

    /// Test the typed key / API (Claude with a key, another API) or the Claude CLI: one fixed line, nothing from this PC.
    pub fn ws_test(&mut self) {
        let c = self.work_setup.choices.clone();
        if c.brain == Brain::ClaudeCli {
            self.claude_test_run();
            return;
        }
        let (kind, base, model) = match c.brain {
            Brain::ClaudeKey => (Kind::Anthropic, None, c.claude_model.clone()),
            Brain::Compatible => (Kind::OpenAiCompatible, Some(c.base_url.trim().trim_end_matches('/').to_string()), c.model.trim().to_string()),
            _ => return,
        };
        // a throw-away provider: nothing is saved by a test
        let mut cfg = AiConfig::default();
        let mut p = Provider::new("test", kind);
        p.model = model;
        if base.is_some() {
            p.base_url = base;
        }
        let typed = Some(Secret::new(&self.work_setup.key)).filter(|k| !k.is_empty());
        // no key typed: the one already in ai.toml for that kind
        let saved = self.ai.config.providers.iter().find(|x| x.kind == kind && x.api_key.is_some()).and_then(|x| x.api_key.clone());
        p.api_key = typed.or(saved);
        cfg.providers.push(p);
        let r = match cfg.resolve_provider("test", &|k| (self.env)(k), None) {
            Ok(r) => r,
            Err(e) => {
                self.work_setup.test = Some(Err(e));
                return;
            }
        };
        let req = crate::llm::Request::ping(crate::llm::Backend::from_resolved(&r), r.api_key.clone(), self.paths.state_dir.join("ai"));
        let t = self.autofix.transport.clone();
        self.work_setup.testing = true;
        self.work_setup.test = None;
        let t0 = std::time::Instant::now();
        log::info!("Work setup: connection test ({}, {})", r.kind.id(), r.model);
        self.task(None, move || {
            let res = t.send(&req).map(|x| format!("OK: {} answered in {:.1} s", x.model, t0.elapsed().as_secs_f32())).map_err(|e| AppCore::ai_plain_error(&e));
            Box::new(move |c: &mut AppCore| {
                c.work_setup.testing = false;
                c.work_setup.test = Some(res);
            })
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(t: &str) -> AiConfig {
        AiConfig::parse(t).unwrap()
    }

    fn here_all() -> OnDisk {
        OnDisk { runtime: true, laya: true, lara: true, qwen4b: true, parakeet: true, whisper: true, ai_bin: Some("/b/omarchyvr-ai".into()), claude: None }
    }

    #[test]
    fn each_answer_writes_the_shared_settings() {
        // the Claude CLI: no key anywhere, no local model, no Lara
        let mut c = cfg("");
        apply(&Choices::default(), None, &mut c).unwrap();
        assert_eq!(c.provider_for(Feature::Voice).unwrap().kind, Kind::ClaudeCli);
        assert!(!c.ai_servers_wanted() && c.voice_local_models == Some(false));
        // the Claude CLI with Lara: Lara goes first, Claude Code gets what it does not handle
        let mut c = cfg("");
        apply(&Choices { lara: true, ..Choices::default() }, None, &mut c).unwrap();
        assert!(c.lara_with_online() && c.ai_servers_wanted() && !c.local_models_on());
        // Claude with a key + Lara: the key is in the provider (mode 600 file), Lara sorts
        let mut c = cfg("");
        let ch = Choices { brain: Brain::ClaudeKey, lara: true, ..Choices::default() };
        apply(&ch, Some(Secret::new("sk-ant-TESTNOTAKEY")), &mut c).unwrap();
        let p = c.provider_for(Feature::Voice).unwrap();
        assert_eq!((p.kind, p.api_key.as_ref().map(|k| k.expose())), (Kind::Anthropic, Some("sk-ant-TESTNOTAKEY")));
        assert!(c.lara_with_online() && c.ai_servers_wanted() && !c.local_models_on());
        // no key typed and none saved: refused
        assert!(apply(&ch, None, &mut cfg("")).is_err());
        // a key already saved: kept when the field is empty
        let mut c2 = c.clone();
        apply(&ch, None, &mut c2).unwrap();
        assert_eq!(c2.provider_for(Feature::Voice).unwrap().api_key.as_ref().unwrap().expose(), "sk-ant-TESTNOTAKEY");
        // the local model on the processor, with Lara: Low (everything on the processor)
        let mut c = cfg("");
        apply(&Choices { brain: Brain::Local, lara: true, ..Choices::default() }, None, &mut c).unwrap();
        assert!(c.local_models_on() && c.voice_local == VoiceLocal::TwoStage);
        assert_eq!((c.preset(), c.two_stage().gpu_layers), (Some(ai::Preset::Low), Some(0)));
        // ... on the graphics card: Medium
        apply(&Choices { brain: Brain::Local, lara: true, local_gpu: true, ..Choices::default() }, None, &mut c).unwrap();
        assert_eq!((c.preset(), c.two_stage().gpu_layers), (Some(ai::Preset::Medium), None));
        // without Lara: one model, the processor by default
        apply(&Choices { brain: Brain::Local, ..Choices::default() }, None, &mut c).unwrap();
        assert_eq!((c.voice_local, c.two_stage().gpu_layers), (VoiceLocal::Simple, Some(0)));
        assert!(!c.lara_on());
        // another compatible API: base URL, model, key
        let mut c = cfg("");
        apply(&Choices { brain: Brain::Compatible, base_url: "https://api.example.com/v1/".into(), model: "m-1".into(), ..Choices::default() }, Some(Secret::new("k-TEST")), &mut c).unwrap();
        let p = c.provider_for(Feature::Voice).unwrap();
        assert_eq!((p.base(), p.model.as_str(), p.api_key.as_ref().map(|k| k.expose())), (Some("https://api.example.com/v1"), "m-1", Some("k-TEST")));
        // none: the built-in commands only
        let mut c = cfg("");
        apply(&Choices { brain: Brain::None, ..Choices::default() }, None, &mut c).unwrap();
        assert_eq!(c.voice_ai, VoiceAi::Off);
        // speech on the graphics card / the processor
        apply(&Choices { speech: Speech::Gpu, ..Choices::default() }, None, &mut c).unwrap();
        assert_eq!(c.asr, Asr::Gpu);
        assert_eq!(c.voice_ai, VoiceAi::Fallback, "a brain again: voice AI back on");
        apply(&Choices::default(), None, &mut c).unwrap();
        assert_eq!(c.asr, Asr::Cpu);
    }

    #[test]
    fn other_providers_and_their_keys_stay() {
        let t = "[use]\nautofix = \"openai\"\n[providers.openai]\nkind = \"openai\"\nmodel = \"gpt-x\"\napi_key = \"sk-KEEPME\"\n[weird]\nx = 1\n";
        let mut c = cfg(t);
        apply(&Choices { brain: Brain::ClaudeKey, ..Choices::default() }, Some(Secret::new("sk-ant-NEW")), &mut c).unwrap();
        let back = AiConfig::parse(&c.render()).unwrap();
        assert_eq!(back.provider("openai").unwrap().api_key.as_ref().unwrap().expose(), "sk-KEEPME");
        assert_eq!(back.uses.get(&Feature::Autofix).map(String::as_str), Some("openai"));
        assert_eq!(back.other_sections.len(), 1, "unknown sections survive");
        // a provider named "claude" of another kind is not overwritten: the key goes into "claude-2"
        let mut c = cfg("[providers.claude]\nkind = \"openai-compatible\"\nbase_url = \"http://127.0.0.1:1/v1\"\nmodel = \"x\"\n");
        apply(&Choices { brain: Brain::ClaudeKey, ..Choices::default() }, Some(Secret::new("sk-ant-NEW")), &mut c).unwrap();
        assert_eq!(c.provider("claude").unwrap().kind, Kind::OpenAiCompatible);
        assert_eq!(c.provider_for(Feature::Voice).unwrap().name, "claude-2");
    }

    #[test]
    fn downloads_reuse_what_is_on_disk() {
        let none = OnDisk { ai_bin: Some("/b".into()), ..OnDisk::default() };
        let local_lara = Choices { brain: Brain::Local, lara: true, ..Choices::default() };
        assert_eq!(downloads(&local_lara, &none), vec!["runtime", "lara", "qwen3.5-4b"]);
        assert_eq!(gb(bytes_of(&downloads(&local_lara, &none))), "5.3 GB");
        assert_eq!(gb(30_416_480), "29 MB", "small files in MB");
        assert_eq!(plain_download_error("x failed (exit status: 1); press Download again to retry (it resumes)"), "x failed (exit status: 1)");
        assert!(downloads(&local_lara, &here_all()).is_empty(), "everything already here: nothing is fetched");
        // Lara with Claude (key or Claude Code): Lara only
        assert_eq!(downloads(&Choices { brain: Brain::ClaudeKey, lara: true, ..Choices::default() }, &none), vec!["lara"]);
        assert_eq!(downloads(&Choices { brain: Brain::ClaudeCli, lara: true, ..Choices::default() }, &none), vec!["lara"]);
        assert!(downloads(&Choices::default(), &none).is_empty(), "the default (Claude CLI, CPU speech) downloads nothing");
    }

    #[test]
    fn problems_in_plain_words() {
        let d = here_all();
        assert!(problem(&Choices { brain: Brain::ClaudeKey, ..Choices::default() }, "", false, &d).unwrap().contains("API key"));
        assert_eq!(problem(&Choices { brain: Brain::ClaudeKey, ..Choices::default() }, "", true, &d), None, "a saved key is enough");
        let api = Choices { brain: Brain::Compatible, base_url: "ftp://x".into(), model: "m".into(), ..Choices::default() };
        assert!(problem(&api, "", false, &d).unwrap().contains("base URL"));
        assert!(problem(&Choices { base_url: "https://x/v1".into(), model: String::new(), ..api.clone() }, "", false, &d).unwrap().contains("model"));
        assert_eq!(problem(&Choices { base_url: "https://x/v1".into(), ..api }, "", false, &d), None, "a key is optional (some servers need none)");
        let no_whisper = OnDisk { whisper: false, ..here_all() };
        let gpu_local = Choices { brain: Brain::Local, speech: Speech::Gpu, ..Choices::default() };
        assert!(problem(&gpu_local, "", false, &no_whisper).unwrap().contains("not installed"));
        assert_eq!(problem(&gpu_local, "", false, &d), None);
        // the graphics-card speech runs only next to the local model
        assert!(problem(&Choices { speech: Speech::Gpu, ..Choices::default() }, "", false, &d).unwrap().contains("only with the model on this PC"));
        let no_bin = OnDisk { ai_bin: None, runtime: false, qwen4b: false, ..OnDisk::default() };
        assert!(problem(&Choices { brain: Brain::Local, ..Choices::default() }, "", false, &no_bin).unwrap().contains("omarchyvr-ai"));
    }

    #[test]
    fn the_progress_protocol_of_omarchyvr_ai_get() {
        let mut s = GetState::default();
        for l in ["get plan lara 3670016000 Lara, the router", "get plan qwen3.5-4b 0 Qwen3.5-4B", "get note lara downloading the Laya checkpoint", "get progress lara 1835008000"] {
            apply_line(&mut s, l);
        }
        assert_eq!(s.items.len(), 2);
        assert_eq!((s.items[0].state.clone(), s.items[1].state.clone()), (ItemState::Running, ItemState::Here), "0 bytes = already on this PC");
        assert!((s.fraction() - 0.5).abs() < 0.01, "{}", s.fraction());
        apply_line(&mut s, "get done lara");
        apply_line(&mut s, "get done qwen3.5-4b");
        apply_line(&mut s, "get all done");
        assert_eq!(s.finished, Some(Ok(())));
        assert_eq!(s.fraction(), 1.0);
        // an error, a cancel
        let mut e = GetState::default();
        apply_line(&mut e, "get plan runtime 30416480 llama.cpp");
        apply_line(&mut e, "get error runtime the download stopped (exit status: 6); check the internet connection");
        assert!(matches!(&e.items[0].state, ItemState::Failed(m) if m.contains("internet")));
        assert!(matches!(&e.finished, Some(Err(m)) if m.contains("internet")));
        let mut c = GetState::default();
        apply_line(&mut c, "get plan lara 100 Lara");
        apply_line(&mut c, "get cancelled lara");
        assert_eq!(c.items[0].state, ItemState::Cancelled);
        assert_eq!(c.finished, Some(Err(omarchyvr_ai::CANCELLED.into())));
        // noise is ignored
        apply_line(&mut c, "Lara: tuning to the voice commands in the background");
        assert_eq!(c.items.len(), 1);
        // a note belongs to the item running; with none running, the next one waiting starts
        let mut n = GetState::default();
        for l in ["get plan runtime 100 llama.cpp", "get plan lara 200 Lara", "get progress runtime 50", "get note installing"] {
            apply_line(&mut n, l);
        }
        assert_eq!((n.items[0].state.clone(), n.items[1].state.clone()), (ItemState::Running, ItemState::Waiting));
        apply_line(&mut n, "get done runtime");
        apply_line(&mut n, "get note lara installing the Python packages");
        assert_eq!(n.items[1].state, ItemState::Running);
    }

    #[test]
    fn the_typed_key_never_shows_in_debug_output() {
        let mut v = WorkSetupView { key: "sk-ant-SECRET-TEST".into(), ..Default::default() };
        let d = format!("{v:?}");
        assert!(!d.contains("SECRET") && d.contains("<hidden>"), "{d}");
        v.wipe_key();
        assert!(v.key.is_empty());
    }

    fn store(tag: &str) -> omarchyvr_config::Store {
        let d = std::env::temp_dir().join(format!("pimax-worksetup-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        omarchyvr_config::Store::new(d.join("config"), d.join("cache"))
    }

    #[test]
    fn an_ai_toml_with_a_brain_counts_the_questions_as_answered() {
        let d = std::env::temp_dir().join(format!("pimax-worksetup-aicfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("ai.toml");
        assert!(!ai_configured(&f), "no file: not configured");
        std::fs::write(&f, "# nothing\n").unwrap();
        assert!(!ai_configured(&f), "an empty file: not configured");
        std::fs::write(&f, "[use]\nvoice = \"claude-cli\"\n").unwrap();
        assert!(ai_configured(&f), "a voice brain: configured");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_first_work_asks_the_questions_then_starts_and_play_never_asks() {
        let mut a = AppCore::demo(false, store("first"));
        a.consent_force = true; // the demo kernel log has WayVR Xid 31 faults
        a.work_setup.record.done = false;
        a.play();
        assert!(a.message.starts_with("demo mode: would run Start VR") || a.message.starts_with("demo mode: would run Start the VR home"), "Play never shows the Work questions: {}", a.message);
        assert_eq!(a.page, Page::Desk);
        a.work();
        assert_eq!(a.page, Page::WorkSetup, "the first Work: the questions first");
        assert!(a.work_setup.start_after && !a.message.contains("Start desk mode"), "{}", a.message);
        assert!(a.work_setup.on_disk.is_some());
        // the defaults: nothing to download; saved, then Work starts by itself
        a.work_setup.choices = Choices::default();
        a.ws_next();
        assert_eq!(a.work_setup.step, Step::Speech);
        a.ws_finish();
        assert!(a.work_setup_done(), "{:?}", a.work_setup.error);
        assert_eq!(a.page, Page::Desk);
        assert!(a.message.starts_with("demo mode: would run Start desk mode"), "{}", a.message);
        assert_eq!(a.ai.config.provider_for(Feature::Voice).unwrap().kind, Kind::ClaudeCli, "the answer is in the shared settings");
        // the next Work starts at once
        a.message.clear();
        a.work();
        assert!(a.message.starts_with("demo mode: would run Start desk mode"), "{}", a.message);
    }

    #[test]
    fn downloads_show_progress_cancel_keeps_nothing_saved_and_close_goes_back() {
        let mut a = AppCore::demo(false, store("get"));
        a.work_setup.record.done = false;
        a.work();
        a.work_setup.choices = Choices { brain: Brain::Local, lara: true, ..Choices::default() };
        assert_eq!(a.work_setup.needed(), vec!["runtime", "lara", "qwen3.5-4b"]);
        a.ws_next();
        a.ws_finish();
        assert_eq!(a.work_setup.step, Step::Get);
        {
            let st = a.work_setup.get.clone().unwrap();
            let s = st.lock().unwrap();
            assert_eq!(s.items.len(), 3);
            assert!(s.fraction() > 0.0 && s.fraction() < 1.0, "{}", s.fraction());
        }
        a.ws_cancel();
        a.poll();
        assert!(a.work_setup.error.as_deref().is_some_and(|e| e.contains("Cancelled")), "{:?}", a.work_setup.error);
        assert!(!a.work_setup_done(), "a cancel saves nothing");
        a.ws_back();
        assert_eq!(a.work_setup.step, Step::Speech);
        // a key typed and closed: wiped, nothing saved, back where the user came from
        a.work_setup.key = "sk-ant-TEST".into();
        a.ws_close();
        assert!(a.work_setup.key.is_empty() && !a.work_setup_done());
        assert_eq!(a.page, Page::Desk);
    }

    #[test]
    fn a_key_is_saved_only_in_ai_toml_and_never_logged() {
        let mut a = AppCore::demo(false, store("key"));
        a.open_work_setup(false);
        // the demo settings hold a Claude key already: start without one
        a.ai.config.providers.retain(|p| p.kind != Kind::Anthropic);
        a.work_setup.choices = Choices { brain: Brain::ClaudeKey, ..Choices::default() };
        a.ws_next();
        assert!(a.work_setup.error.as_deref().is_some_and(|e| e.contains("API key")), "no key, none saved: {:?}", a.work_setup.error);
        a.work_setup.key = "sk-ant-TESTKEY-NOT-REAL".into();
        a.ws_next();
        a.ws_finish();
        assert!(a.work_setup.key.is_empty(), "the field is wiped once saved");
        let p = a.ai.config.provider_for(Feature::Voice).unwrap();
        assert_eq!(p.api_key.as_ref().map(|k| k.expose()), Some("sk-ant-TESTKEY-NOT-REAL"));
        // nothing the app keeps for people shows it: the log, the message, the record
        assert!(!a.log.iter().any(|l| l.contains("TESTKEY")) && !a.message.contains("TESTKEY"));
        assert!(!a.work_setup.record.render().contains("TESTKEY"));
        assert!(!format!("{:?}", a.work_setup).contains("TESTKEY"));
        // the real file is written with mode 600 by omarchyvr_config::ai (tested there); here: the round trip
        let d = std::env::temp_dir().join(format!("pimax-worksetup-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let f = d.join("omarchyvr/ai.toml");
        a.ai.config.save_to(&f).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_wizard_opens_with_the_current_setup() {
        let rec = WorkSetup::default();
        assert_eq!(choices_from(&cfg(""), &rec).brain, Brain::ClaudeCli, "nothing set: the Claude CLI, the default brain");
        let key = "voice_lara = true\nvoice_local_models = false\n[use]\nvoice = \"claude\"\n[providers.claude]\nkind = \"anthropic\"\napi_key = \"sk-ant-X\"\nmodel = \"claude-sonnet-5-5\"\n";
        let c = choices_from(&cfg(key), &rec);
        assert_eq!((c.brain, c.lara, c.claude_model.as_str()), (Brain::ClaudeKey, true, "claude-sonnet-5-5"));
        let local = "voice_local = \"two-stage\"\nvoice_local_models = true\nasr = \"gpu\"\n[use]\nvoice = \"local\"\n[providers.local]\nkind = \"openai-compatible\"\nbase_url = \"http://127.0.0.1:8080/v1\"\nmodel = \"pimax-local\"\n";
        let c = choices_from(&cfg(local), &rec);
        assert_eq!((c.brain, c.lara, c.speech), (Brain::Local, true, Speech::Gpu));
        assert_eq!(choices_from(&cfg("voice_ai = \"off\"\n"), &rec).brain, Brain::None);
        // asr = "gpu" with an online brain runs on the processor: the wizard says so
        assert_eq!(choices_from(&cfg(&format!("asr = \"gpu\"\n{key}")), &rec).speech, Speech::Cpu);
    }
}
