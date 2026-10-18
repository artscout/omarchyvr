//! AI providers shared by the Pimax app (AI tab, Autofix) and voice control: `~/.config/omarchyvr/ai.toml`.
//!
//! ```toml
//! # no file, or no provider chosen for voice: the built-in `claude-cli` provider (the installed Claude Code CLI,
//! # model "haiku", no key) answers voice commands that the offline matcher does not understand.
//! default = "claude"              # the provider every feature uses unless [use] says otherwise
//! voice_ai = "fallback"           # off | fallback (when the offline matcher has no answer) | always
//! strip_titles = false            # privacy: window titles are not sent to a provider
//! voice_confirm = "none"          # none (default) | dangerous | always: when a voice command waits for "yes"
//! voice_listen = "long"           # normal | long (default) | very-long: how long voice waits after you stop talking
//! voice_ai_timeout_s = 4          # optional: how long voice waits for an answer (default 4 local, 8 remote)
//! voice_ai_gaze = true            # optional: send what you look at with a voice request (default true)
//! voice_ai_log = true             # optional: ~/.local/state/omarchyvr/logs/ai.log (default true)
//! voice_ai_fast_path = false      # optional: true = a short command the rules / matcher settle runs without the AI
//!                                 # (default false: with the AI on, the AI decides every command, docs/VOICE.md)
//!
//! [use]                           # optional per-feature choice
//! voice = "claude"
//! autofix = "claude"
//!
//! [providers.claude-cli]          # the default for voice: runs `claude -p` (the user's own login, no API key)
//! kind = "claude-cli"
//! model = "haiku"                 # a CLI alias or id: haiku | sonnet | claude-haiku-5-5 ...
//! command = "claude"              # optional: the binary (default: `claude` on PATH, then ~/.local/bin/claude)
//!
//! [providers.claude]
//! kind = "anthropic"              # anthropic | openai | openai-compatible | claude-cli
//! model = "claude-haiku-5-5"
//! api_key = "sk-ant-..."          # or: api_key_env = "ANTHROPIC_API_KEY"
//!
//! [providers.openai]
//! kind = "openai"
//! model = "gpt-..."
//! api_key = "..."
//! base_url = "https://api.openai.com/v1"
//!
//! [providers.local]               # ollama, llama-server, LM Studio: no key
//! kind = "openai-compatible"
//! base_url = "http://127.0.0.1:11434/v1"
//! model = "qwen3:8b"
//! ```
//!
//! Keys, `[use]` entries and sections this version does not know are kept as they are when the file is saved.
//!
//! The file holds secrets: it is written atomically with mode 0600 (its directory 0700 when created here), and a load
//! warns when group or others can read it. Keys never appear in `Debug`, in [`AiConfig::redacted`] or in errors.
//! Endpoints: Anthropic `<base_url>/v1/messages` (base `https://api.anthropic.com`), OpenAI and compatible servers
//! `<base_url>/chat/completions` (the base includes `/v1`); see [`Resolved::endpoint`].
use std::collections::BTreeMap;
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Claude models offered (id, name); the first one is the voice default.
pub const CLAUDE_MODELS: [(&str, &str); 3] = [
    ("claude-haiku-5-5", "Claude Haiku 5.5 (fast)"),
    ("claude-sonnet-5-5", "Claude Sonnet 5.5"),
    ("claude-opus-5-5", "Claude Opus 5.5 (most capable)"),
];

/// Models offered for the Claude CLI provider (alias, name); the first is the default.
pub const CLAUDE_CLI_MODELS: [(&str, &str); 2] = [("haiku", "Claude Haiku (fast)"), ("sonnet", "Claude Sonnet (more capable)")];
/// The name of the built-in Claude CLI provider.
pub const CLAUDE_CLI_NAME: &str = "claude-cli";

/// The Anthropic API base.
pub const ANTHROPIC_BASE: &str = "https://api.anthropic.com";
/// The OpenAI API base.
pub const OPENAI_BASE: &str = "https://api.openai.com/v1";
/// OpenRouter's OpenAI-compatible API (one key, many models, the price in every answer).
pub const OPENROUTER_BASE: &str = "https://openrouter.ai/api/v1";
/// The OpenRouter model voice control was tuned with: a router that picks the model per request. It sorts a voice
/// command into a category (the intent model).
pub const OPENROUTER_VOICE_MODEL: &str = "typesafe/jev-router";
/// The OpenRouter models offered to act on a voice command with tools (the action model); the first is the default
/// (benchmark in docs/AI.md section 14).
pub const OPENROUTER_ACTION_MODELS: [&str; 2] = ["openai/gpt-6-luna", "anthropic/claude-haiku-5.5"];
/// ollama's OpenAI-compatible API.
pub const OLLAMA_BASE: &str = "http://127.0.0.1:11434/v1";
/// llama.cpp `llama-server`'s OpenAI-compatible API.
pub const LLAMA_SERVER_BASE: &str = "http://127.0.0.1:8080/v1";

/// How a provider is spoken to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    /// Anthropic Messages API (`x-api-key`).
    Anthropic,
    /// OpenAI Chat Completions (`Authorization: Bearer`).
    OpenAi,
    /// Any server with the OpenAI Chat Completions shape (ollama, llama-server, LM Studio, ...); key optional.
    OpenAiCompatible,
    /// The user's installed Claude Code CLI (`claude -p`): their own login, no API key, no URL.
    ClaudeCli,
}

impl Kind {
    /// Every kind.
    pub const ALL: [Kind; 4] = [Kind::Anthropic, Kind::OpenAi, Kind::OpenAiCompatible, Kind::ClaudeCli];

    /// The value in the file.
    pub fn id(self) -> &'static str {
        match self {
            Kind::Anthropic => "anthropic",
            Kind::OpenAi => "openai",
            Kind::OpenAiCompatible => "openai-compatible",
            Kind::ClaudeCli => "claude-cli",
        }
    }

    /// From the file.
    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.id() == s)
    }

    /// For people.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Anthropic => "Claude (Anthropic)",
            Kind::OpenAi => "OpenAI",
            Kind::OpenAiCompatible => "Local or other (OpenAI-compatible)",
            Kind::ClaudeCli => "Claude (CLI, no key)",
        }
    }

    /// The base URL when the file sets none (none for compatible servers: they need one).
    pub fn default_base(self) -> Option<&'static str> {
        match self {
            Kind::Anthropic => Some(ANTHROPIC_BASE),
            Kind::OpenAi => Some(OPENAI_BASE),
            Kind::OpenAiCompatible | Kind::ClaudeCli => None,
        }
    }

    /// A key is required.
    pub fn needs_key(self) -> bool {
        !matches!(self, Kind::OpenAiCompatible | Kind::ClaudeCli)
    }

    /// The environment variable a key usually comes from.
    pub fn usual_env(self) -> Option<&'static str> {
        match self {
            Kind::Anthropic => Some("ANTHROPIC_API_KEY"),
            Kind::OpenAi => Some("OPENAI_API_KEY"),
            Kind::OpenAiCompatible | Kind::ClaudeCli => None,
        }
    }
}

/// A key: never printed (Debug and Display show nothing of it), overwritten when dropped.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wrap a key (surrounding white space removed).
    pub fn new(s: &str) -> Secret {
        Secret(s.trim().to_string())
    }

    /// The key (for a request header only).
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Length in characters.
    pub fn len(&self) -> usize {
        self.0.chars().count()
    }

    /// Empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<hidden>)")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<hidden, {} characters>", self.len())
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        let mut b = std::mem::take(&mut self.0).into_bytes();
        b.iter_mut().for_each(|x| *x = 0);
        std::hint::black_box(&b);
    }
}

/// How voice control uses a language model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VoiceAi {
    /// Never: only the offline command matcher.
    Off,
    /// Only when the offline matcher has no answer (the default).
    #[default]
    Fallback,
    /// Every command goes to the model.
    Always,
}

impl VoiceAi {
    /// Every mode.
    pub const ALL: [VoiceAi; 3] = [VoiceAi::Off, VoiceAi::Fallback, VoiceAi::Always];

    /// The value in the file.
    pub fn id(self) -> &'static str {
        match self {
            VoiceAi::Off => "off",
            VoiceAi::Fallback => "fallback",
            VoiceAi::Always => "always",
        }
    }

    /// From the file.
    pub fn parse(s: &str) -> Option<VoiceAi> {
        VoiceAi::ALL.into_iter().find(|v| v.id() == s)
    }

    /// For people.
    pub fn label(self) -> &'static str {
        match self {
            VoiceAi::Off => "Off (offline commands only)",
            VoiceAi::Fallback => "When a command is not understood",
            VoiceAi::Always => "Always",
        }
    }
}

/// How long the voice listener waits after you stop talking (`voice_listen`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VoiceListen {
    /// The old timings: quick, but long commands get cut at a pause.
    Normal,
    /// A bigger buffer (the default, user request 2026-10-18).
    #[default]
    Long,
    /// For long commands of many parts with natural pauses.
    VeryLong,
}

impl VoiceListen {
    pub const ALL: [VoiceListen; 3] = [VoiceListen::Normal, VoiceListen::Long, VoiceListen::VeryLong];

    pub fn id(self) -> &'static str {
        match self {
            VoiceListen::Normal => "normal",
            VoiceListen::Long => "long",
            VoiceListen::VeryLong => "very-long",
        }
    }

    pub fn parse(s: &str) -> Option<VoiceListen> {
        VoiceListen::ALL.into_iter().find(|v| v.id() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            VoiceListen::Normal => "Quick",
            VoiceListen::Long => "Long (default)",
            VoiceListen::VeryLong => "Very long",
        }
    }

    /// Silence that ends an utterance, ms (`--endpoint-ms`).
    pub fn endpoint_ms(self) -> u32 {
        match self {
            VoiceListen::Normal => 480,
            VoiceListen::Long => 750,
            VoiceListen::VeryLong => 1100,
        }
    }

    /// Silence that ends an utterance after a dangling word ("and", "on", ...), ms (`--hangover-ms`).
    pub fn long_hangover_ms(self) -> u32 {
        match self {
            VoiceListen::Normal => 1120,
            VoiceListen::Long => 1500,
            VoiceListen::VeryLong => 2000,
        }
    }

    /// How long a held cut-off sentence waits for the next utterance, seconds.
    pub fn followup_s(self) -> f64 {
        match self {
            VoiceListen::Normal => 6.0,
            VoiceListen::Long => 8.0,
            VoiceListen::VeryLong => 10.0,
        }
    }
}

/// When a voice command waits for a spoken "yes" (`voice_confirm`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VoiceConfirm {
    /// Never: every command runs at once (the default, user decision 2026-10-15).
    #[default]
    None,
    /// Only dangerous commands: lock, close a window, typing or Enter into a terminal, power / session stop.
    Dangerous,
    /// Every command.
    Always,
}

impl VoiceConfirm {
    pub const ALL: [VoiceConfirm; 3] = [VoiceConfirm::None, VoiceConfirm::Dangerous, VoiceConfirm::Always];

    pub fn id(self) -> &'static str {
        match self {
            VoiceConfirm::None => "none",
            VoiceConfirm::Dangerous => "dangerous",
            VoiceConfirm::Always => "always",
        }
    }

    pub fn parse(s: &str) -> Option<VoiceConfirm> {
        VoiceConfirm::ALL.into_iter().find(|v| v.id() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            VoiceConfirm::None => "Never ask",
            VoiceConfirm::Dangerous => "Ask for dangerous commands",
            VoiceConfirm::Always => "Ask for every command",
        }
    }

    /// Does a command (dangerous or not) wait for "yes"?
    pub fn asks(self, dangerous: bool) -> bool {
        match self {
            VoiceConfirm::None => false,
            VoiceConfirm::Dangerous => dangerous,
            VoiceConfirm::Always => true,
        }
    }
}

/// How voice control uses the AI on this PC (`voice_local`; docs/AI.md section 16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VoiceLocal {
    /// One model (`omarchyvr-ai serve`, port 8080) answers in one short request (the default).
    #[default]
    Simple,
    /// Two stages, like Jev online: a small router ("Lara") sorts the command into a category, then the model on
    /// port 8080 (on the GPU) acts with only that category's tools.
    TwoStage,
}

impl VoiceLocal {
    pub const ALL: [VoiceLocal; 2] = [VoiceLocal::Simple, VoiceLocal::TwoStage];

    pub fn id(self) -> &'static str {
        match self {
            VoiceLocal::Simple => "simple",
            VoiceLocal::TwoStage => "two-stage",
        }
    }

    pub fn parse(s: &str) -> Option<VoiceLocal> {
        match s.trim() {
            "simple" | "one" | "compact" => Some(VoiceLocal::Simple),
            "two-stage" | "two_stage" | "twostage" => Some(VoiceLocal::TwoStage),
            _ => None,
        }
    }
}

/// Speech recognition (`asr`; docs/AI.md sections 17 and 19): Parakeet on the processor (the default), or whisper
/// large-v3-turbo on the graphics card (the "Speech on graphics card" switch).
///
/// Since 2026-10-16 the processor is the default: the GPU speech server shared the card with the AI models and made
/// Monado skip 8.8% of its frames (the VR screens shook). A saved `asr = "auto"` (or no key) means the processor;
/// only an explicit `asr = "gpu"` keeps the graphics card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Asr {
    /// Parakeet on the processor with hotwords (no video memory, no server).
    #[default]
    Cpu,
    /// whisper large-v3-turbo on the graphics card (whisper-server, port 8082, Work only).
    Gpu,
}

/// whisper-server's port.
pub const ASR_PORT: u16 = 8082;
/// What the large model takes on the graphics card (MiB, measured: 835).
pub const ASR_VRAM_MIB: u32 = 900;

impl Asr {
    pub const ALL: [Asr; 2] = [Asr::Cpu, Asr::Gpu];

    pub fn id(self) -> &'static str {
        match self {
            Asr::Cpu => "cpu",
            Asr::Gpu => "gpu",
        }
    }

    /// From the file; the old default `auto` is the processor now (migration).
    pub fn parse(s: &str) -> Option<Asr> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" | "cpu" | "small" | "processor" => Some(Asr::Cpu),
            "gpu" | "large" | "graphics" => Some(Asr::Gpu),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Asr::Cpu => "Processor",
            Asr::Gpu => "Graphics card",
        }
    }

    /// The measured numbers for the app (60 spoken commands, clean and with noise; docs/AI.md section 17).
    pub fn numbers(self) -> &'static str {
        match self {
            Asr::Cpu => "Parakeet + hotwords: 13.3% word errors in noise (2.7% clean), 0.14 s, no video memory, English and Spanish",
            Asr::Gpu => "Whisper large-v3-turbo: 10.3% word errors in noise (6.4% clean), 0.04 s, 0.9 GB of video memory, English only",
        }
    }
}

/// The "Speech on graphics card" switch.
impl AiConfig {
    pub fn asr_on_gpu(&self) -> bool {
        self.asr == Asr::Gpu
    }
}

/// What the speech recognition does now and why, in plain words ([`asr_plan`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsrPlan {
    /// The large model runs on the graphics card (whisper-server is wanted).
    pub gpu: bool,
    pub why: String,
}

/// Decide processor / graphics card. `local_on`: the Local models switch; `free_mib`: free video memory now with the
/// ASR server's own share counted as free (None: unknown, nvidia-smi missing).
pub fn asr_plan(choice: Asr, local_on: bool, free_mib: Option<u32>) -> AsrPlan {
    let gb = |m: u32| format!("{:.1} GB", m as f64 / 1024.0);
    match choice {
        Asr::Cpu => AsrPlan { gpu: false, why: "Speech runs on the processor (the graphics card stays free for VR)".into() },
        Asr::Gpu if !local_on => AsrPlan { gpu: false, why: "The graphics-card model runs with the local models: turn Local models On (Work only); the processor answers meanwhile".into() },
        Asr::Gpu => match free_mib {
            Some(f) if f < ASR_VRAM_MIB + 256 => AsrPlan { gpu: false, why: format!("Not enough video memory for the large model ({} free, it needs {}): the processor answers", gb(f), gb(ASR_VRAM_MIB)) },
            _ => AsrPlan { gpu: true, why: "Large model on the graphics card (your choice; it can make VR stutter)".into() },
        },
    }
}

/// Where a local model of the two-stage pipeline runs and what it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalEngine {
    /// Laya (Python, `laya-serve`, TypeSafe Jev's `/v1/systemone` protocol): an encoder classifier, one forward pass.
    Laya,
    /// A GGUF model on llama.cpp's `llama-server`.
    Llama,
}

/// A model the two-stage pipeline can use (the download pins are in `omarchyvr-ai`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalModel {
    /// The id in ai.toml (`[local_two_stage] router` / `action`).
    pub id: &'static str,
    /// For people.
    pub label: &'static str,
    pub engine: LocalEngine,
    /// Download size (MiB), runtime included for Laya.
    pub download_mib: u32,
    /// Memory while it runs (MiB): VRAM for an action model on the GPU, RAM for a router on the CPU.
    pub memory_mib: u32,
    /// The weights' licence.
    pub licence: &'static str,
    /// One line for the app.
    pub note: &'static str,
    /// The audit's result for this model (docs/AI.md section 16): accuracy and latency, for the pickers.
    pub bench: &'static str,
    /// The recommended choice of its list.
    pub recommended: bool,
}

/// Stage 1 ("Lara"): the routers, best first (benchmark in docs/AI.md section 16). All run on the CPU only.
pub const ROUTER_MODELS: [LocalModel; 3] = [
    LocalModel {
        id: "lara",
        label: "Lara (Laya tuned to your commands)",
        engine: LocalEngine::Laya,
        download_mib: 2760,
        memory_mib: 2000,
        licence: "Apache-2.0 (convaiinnovations/laya, tuned here on the command catalogue; PyTorch CPU)",
        note: "Laya, the open Jev, tuned on this PC to the voice commands (about an hour once, in the background).",
        bench: "94 % of categories right on unseen phrasings (Laya: 57 %); 0.3 s on the processor",
        recommended: true,
    },
    LocalModel {
        id: "laya",
        label: "Laya 421M as published (the open Jev)",
        engine: LocalEngine::Laya,
        download_mib: 2760,
        memory_mib: 2000,
        licence: "Apache-2.0 (convaiinnovations/laya; Python, PyTorch CPU)",
        note: "Open-weights classifier that speaks Jev's protocol; one pass, no text generated; not tuned.",
        bench: "57 % of categories right on unseen phrasings; 0.33 s on the processor",
        recommended: false,
    },
    LocalModel {
        id: "qwen3.5-0.8b",
        label: "Qwen3.5-0.8B (small LLM router)",
        engine: LocalEngine::Llama,
        download_mib: 508,
        memory_mib: 900,
        licence: "Apache-2.0 (Qwen3.5, unsloth GGUF)",
        note: "A small language model that picks the category under a grammar; CPU only.",
        bench: "57 % of categories right; 0.8 s on a busy processor (missed the 1.5 s deadline)",
        recommended: false,
    },
];

/// Stage 2: the action models (llama.cpp, GPU by default), smallest first.
pub const ACTION_MODELS: [LocalModel; 2] = [
    LocalModel {
        id: "qwen3.5-4b",
        label: "Qwen3.5-4B",
        engine: LocalEngine::Llama,
        download_mib: 2614,
        memory_mib: 3100,
        licence: "Apache-2.0 (Qwen3.5, unsloth GGUF)",
        note: "The model of the simple mode, here with tools.",
        bench: "40/40 hard phrases, 458/460 in all with Lara; 0.2 s alone, 0.35 s with Lara",
        recommended: true,
    },
    LocalModel {
        id: "qwen3.5-9b",
        label: "Qwen3.5-9B",
        engine: LocalEngine::Llama,
        download_mib: 4929,
        memory_mib: 5000,
        licence: "Apache-2.0 (Qwen3.5, unsloth GGUF IQ4_XS)",
        note: "Larger and slower; better with unusual wording.",
        bench: "39/40 hard phrases alone, 38/40 with Lara; 0.25 s alone, 0.35 s with Lara",
        recommended: false,
    },
];

/// The agent-mode model (docs/AI.md section 18), an option next to the action model: only multi-step plans (agent mode)
/// use it, on its own port; simple commands stay on the fast path (Lara + the action model).
pub const AGENT_MODELS: [LocalModel; 1] = [LocalModel {
    id: "qwen3.6-27b",
    label: "Qwen3.6-27B (agent mode)",
    engine: LocalEngine::Llama,
    download_mib: 16036,
    memory_mib: 18000,
    licence: "Apache-2.0 (Qwen3.6, unsloth GGUF Q4_K_M)",
    note: "Plans of several steps only; needs about 18 GB of video memory, so Work without a game.",
    bench: "see docs/AI.md section 18",
    recommended: false,
}];

/// Port of the agent-mode model's server.
pub const AGENT_PORT: u16 = 8083;

/// The router's address (stage 1, `omarchyvr-ai serve --slot router`).
pub const ROUTER_BASE: &str = "http://127.0.0.1:8081";

/// A local model by id (router or action).
pub fn local_model(id: &str) -> Option<&'static LocalModel> {
    ROUTER_MODELS.iter().chain(ACTION_MODELS.iter()).chain(AGENT_MODELS.iter()).find(|m| m.id == id)
}

/// The two-stage pipeline on this PC (`[local_two_stage]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalTwoStage {
    /// Stage 1, an id of [`ROUTER_MODELS`].
    pub router: String,
    /// Stage 2, an id of [`ACTION_MODELS`] (served on port 8080 instead of the simple model).
    pub action: String,
    /// Optional agent-mode model, an id of [`AGENT_MODELS`] (None = off, the default).
    pub agent: Option<String>,
    /// GPU layers of the action model: None = all, Some(0) = CPU.
    pub gpu_layers: Option<u32>,
    /// "VRAM for AI": None = Auto (the default: what is free next to the running session, measured at each start and
    /// re-checked while it runs), Some(N) = at most N MiB.
    pub vram_mib: Option<u32>,
    /// The preset the models came from (None: chosen by hand, "Advanced").
    pub preset: Option<Preset>,
}

/// Local AI presets (user decision 2026-10-16: Low / Medium / High, each a router + action model pair from the
/// benchmark of docs/AI.md section 16).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Low,
    Medium,
    High,
}

/// One preset: what it sets and what the benchmark measured (for the app and the watch).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresetInfo {
    pub preset: Preset,
    pub router: &'static str,
    pub action: &'static str,
    /// GPU layers of the action model (Some(0) = the processor).
    pub gpu_layers: Option<u32>,
    pub label: &'static str,
    /// Download, RAM and VRAM in plain words.
    pub size: &'static str,
    /// Speed and accuracy from the audit.
    pub bench: &'static str,
}

/// The presets, Low first; Medium is the default.
pub const PRESETS: [PresetInfo; 3] = [
    PresetInfo {
        preset: Preset::Low,
        router: "lara",
        action: "qwen3.5-4b",
        gpu_layers: Some(0),
        label: "Low: everything on the processor",
        size: "5.3 GB download, about 5 GB of RAM, no video memory",
        bench: "40/40 hard phrases (the 4B on the processor); about 1.2-1.5 s per command",
    },
    PresetInfo {
        preset: Preset::Medium,
        router: "lara",
        action: "qwen3.5-4b",
        gpu_layers: None,
        label: "Medium: Lara + Qwen3.5-4B on the graphics card",
        size: "5.3 GB download, 2 GB of RAM, 3 GB of video memory",
        bench: "40/40 hard phrases, 458/460 in all (plans included); about 0.35 s",
    },
    PresetInfo {
        preset: Preset::High,
        router: "lara",
        action: "qwen3.5-9b",
        gpu_layers: None,
        label: "High: Lara + Qwen3.5-9B on the graphics card",
        size: "7.5 GB download, 2 GB of RAM, 4.7 GB of video memory",
        bench: "38/40 hard phrases; about 0.35 s (no more accurate here than Medium)",
    },
];

impl Preset {
    pub const ALL: [Preset; 3] = [Preset::Low, Preset::Medium, Preset::High];
    pub fn id(self) -> &'static str {
        match self {
            Preset::Low => "low",
            Preset::Medium => "medium",
            Preset::High => "high",
        }
    }
    pub fn parse(s: &str) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.id() == s.trim())
    }
    /// "Low", "Medium", "High".
    pub fn name(self) -> &'static str {
        match self {
            Preset::Low => "Low",
            Preset::Medium => "Medium",
            Preset::High => "High",
        }
    }
    pub fn info(self) -> &'static PresetInfo {
        PRESETS.iter().find(|p| p.preset == self).unwrap_or(&PRESETS[1])
    }
    /// The next one (the watch button cycles).
    pub fn next(self) -> Preset {
        match self {
            Preset::Low => Preset::Medium,
            Preset::Medium => Preset::High,
            Preset::High => Preset::Low,
        }
    }
}

impl LocalTwoStage {
    /// Set the models of a preset (the VRAM limit stays).
    pub fn apply(&mut self, p: Preset) {
        let i = p.info();
        self.router = i.router.into();
        self.action = i.action.into();
        self.gpu_layers = i.gpu_layers;
        self.preset = Some(p);
    }
    /// The preset these models match, if any.
    pub fn matching(&self) -> Option<Preset> {
        PRESETS.iter().find(|i| i.router == self.router && i.action == self.action && i.gpu_layers == self.gpu_layers).map(|i| i.preset)
    }
}

impl Default for LocalTwoStage {
    fn default() -> Self {
        LocalTwoStage { router: ROUTER_MODELS[0].id.into(), action: ACTION_MODELS[0].id.into(), agent: None, gpu_layers: None, vram_mib: None, preset: Some(Preset::Medium) }
    }
}

/// "AI runs on" for voice control (the AI page's first switch; the Work desk's watch button).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunsOn {
    /// This PC: one model, one short request.
    Simple,
    /// This PC: the router "Lara" sorts the command, the model on the GPU acts with that category's tools.
    TwoStage,
    /// Online: Jev sorts, an action model acts (an online provider, OpenRouter first).
    Online,
}

impl RunsOn {
    pub const ALL: [RunsOn; 3] = [RunsOn::Simple, RunsOn::TwoStage, RunsOn::Online];
    pub fn id(self) -> &'static str {
        match self {
            RunsOn::Simple => "simple",
            RunsOn::TwoStage => "two-stage",
            RunsOn::Online => "online",
        }
    }
    pub fn parse(s: &str) -> Option<RunsOn> {
        match s.trim() {
            "simple" | "local" | "this-pc" => Some(RunsOn::Simple),
            "two-stage" | "two_stage" | "lara" => Some(RunsOn::TwoStage),
            "online" | "remote" => Some(RunsOn::Online),
            _ => None,
        }
    }
    /// For the app's selector.
    pub fn label(self) -> &'static str {
        match self {
            RunsOn::Simple => "This PC (simple)",
            RunsOn::TwoStage => "This PC (two-stage: Lara + GPU model)",
            RunsOn::Online => "Online (Jev + action model)",
        }
    }
    /// For the watch button and the captions (short).
    pub fn short(self) -> &'static str {
        match self {
            RunsOn::Simple => "This PC",
            RunsOn::TwoStage => "This PC, 2 stages",
            RunsOn::Online => "Online",
        }
    }
}

/// A feature that uses a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Feature {
    /// Voice control (commands, window control).
    Voice,
    /// Autofix (Setup > Autofix in the app).
    Autofix,
}

impl Feature {
    /// Every feature.
    pub const ALL: [Feature; 2] = [Feature::Voice, Feature::Autofix];

    /// The key in `[use]`.
    pub fn id(self) -> &'static str {
        match self {
            Feature::Voice => "voice",
            Feature::Autofix => "autofix",
        }
    }

    /// For people.
    pub fn label(self) -> &'static str {
        match self {
            Feature::Voice => "Voice control",
            Feature::Autofix => "Autofix",
        }
    }
}

/// One provider (`[providers.<name>]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    /// The table name: letters, digits, `-`, `_`.
    pub name: String,
    /// How it is spoken to.
    pub kind: Kind,
    /// Model id (Claude: one of [`CLAUDE_MODELS`]; others: free text).
    pub model: String,
    /// The key, stored in the file.
    pub api_key: Option<Secret>,
    /// Or the environment variable that holds it.
    pub api_key_env: Option<String>,
    /// Base URL (see [`Kind::default_base`]).
    pub base_url: Option<String>,
    /// Voice control's second step (acting with tools) uses this model when set (`action_model`); `model` then
    /// only sorts the command (the intent model).
    pub action_model: Option<String>,
    /// Claude CLI only: the binary (`command`; None: `claude` on PATH, then the usual install places).
    pub command: Option<String>,
    /// Keys this version does not know, raw values (kept on save).
    pub extra: BTreeMap<String, String>,
}

impl Provider {
    /// A provider with the usual settings of its kind.
    pub fn new(name: &str, kind: Kind) -> Provider {
        Provider {
            name: name.to_string(),
            kind,
            model: match kind {
                Kind::Anthropic => CLAUDE_MODELS[0].0.to_string(),
                Kind::ClaudeCli => CLAUDE_CLI_MODELS[0].0.to_string(),
                _ => String::new(),
            },
            api_key: None,
            api_key_env: None,
            base_url: match kind {
                Kind::OpenAi => Some(OPENAI_BASE.into()),
                Kind::OpenAiCompatible => Some(OLLAMA_BASE.into()),
                Kind::Anthropic | Kind::ClaudeCli => None,
            },
            action_model: None,
            command: None,
            extra: BTreeMap::new(),
        }
    }

    /// The built-in Claude CLI provider (`claude-cli`, model haiku): voice uses it when the file chooses nothing.
    pub fn claude_cli() -> Provider {
        Provider::new(CLAUDE_CLI_NAME, Kind::ClaudeCli)
    }

    /// The binary to run (Claude CLI): `command`, else `claude`.
    pub fn cli_command(&self) -> &str {
        self.command.as_deref().map(str::trim).filter(|c| !c.is_empty()).unwrap_or("claude")
    }

    /// Requests leave this PC (a provider that is not on a loopback address; the Claude CLI always calls Anthropic).
    pub fn is_remote(&self) -> bool {
        self.kind == Kind::ClaudeCli || self.base().is_some_and(|b| !is_loopback(b))
    }

    /// The base URL in use.
    pub fn base(&self) -> Option<&str> {
        self.base_url.as_deref().filter(|b| !b.trim().is_empty()).or(self.kind.default_base())
    }
}

/// Where a resolved key came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySource {
    /// `api_key` in the file.
    File,
    /// An environment variable.
    Env(String),
    /// Given by the caller (a key kept for this session only, never saved).
    Session,
    /// No key (a local server).
    None,
}

/// What a feature should talk to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The provider's name.
    pub provider: String,
    /// How to speak to it.
    pub kind: Kind,
    /// Model id.
    pub model: String,
    /// Base URL.
    pub base_url: String,
    /// The key, if any.
    pub api_key: Option<Secret>,
    /// Where the key came from.
    pub key_source: KeySource,
    /// Voice control's action model (`action_model`), if set.
    pub action_model: Option<String>,
    /// Claude CLI: the binary to run (`command` in the file; None: `claude`).
    pub command: Option<String>,
}

impl Resolved {
    /// The URL a request goes to: Anthropic `<base>/v1/messages`, the others `<base>/chat/completions`. The Claude
    /// CLI has none (its endpoint is the word `claude-cli`).
    pub fn endpoint(&self) -> String {
        let b = self.base_url.trim_end_matches('/');
        match self.kind {
            Kind::ClaudeCli => "claude-cli".to_string(),
            Kind::Anthropic => format!("{b}/v1/messages"),
            _ => format!("{b}/chat/completions"),
        }
    }

    /// Requests stay on this machine (a loopback base URL; never the Claude CLI).
    pub fn local(&self) -> bool {
        self.kind != Kind::ClaudeCli && is_loopback(&self.base_url)
    }
}

/// `http://127.0.0.1...`, `http://localhost...`, `http://[::1]...`.
pub fn is_loopback(url: &str) -> bool {
    let rest = url.strip_prefix("http://").or_else(|| url.strip_prefix("https://")).unwrap_or("");
    let host = rest.split(['/', '?']).next().unwrap_or("");
    let host = if host.starts_with('[') { host.split(']').next().map(|h| format!("{h}]")).unwrap_or_default() } else { host.split(':').next().unwrap_or("").to_string() };
    host == "127.0.0.1" || host == "localhost" || host == "[::1]"
}

/// The whole file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AiConfig {
    /// The default provider.
    pub default: Option<String>,
    /// Per-feature choice (`[use]`).
    pub uses: BTreeMap<Feature, String>,
    /// Providers, in file order.
    pub providers: Vec<Provider>,
    /// How voice control uses a model (`voice_ai`).
    pub voice_ai: VoiceAi,
    /// Privacy: window titles are not sent to a provider (`strip_titles`).
    pub strip_titles: bool,
    /// How long voice control waits for an answer, seconds (`voice_ai_timeout_s`; None: 4 local, 8 remote).
    pub voice_ai_timeout_s: Option<f64>,
    /// When a voice command waits for a spoken "yes" (`voice_confirm`, default none).
    pub voice_confirm: VoiceConfirm,
    /// How long the voice listener waits after you stop talking (`voice_listen`, default long).
    pub voice_listen: VoiceListen,
    /// Send what you look at with a voice request (`voice_ai_gaze`; None: yes).
    pub voice_ai_gaze: Option<bool>,
    /// Log AI requests to `~/.local/state/omarchyvr/logs/ai.log` (`voice_ai_log`; None: yes).
    pub voice_ai_log: Option<bool>,
    /// Keep one Claude CLI process started ahead for the next command (`voice_ai_prewarm`; None: yes).
    pub voice_ai_prewarm: Option<bool>,
    /// With a local voice model: use the online provider when the local server is not running (`voice_online_backup`; None: yes).
    pub voice_online_backup: Option<bool>,
    /// The AI on this PC: one model or two stages (`voice_local`, default simple).
    pub voice_local: VoiceLocal,
    /// The two-stage pipeline's models (`[local_two_stage]`; None: the defaults).
    pub local_two_stage: Option<LocalTwoStage>,
    /// "Local models: On / Off" (`voice_local_models`; None: not set, the models follow `[use] voice`). Off: no local
    /// model is loaded and voice uses the online provider when chosen, else only the offline commands. On: they run
    /// in Work sessions only (omarchyvrd's ai.server; never in Play / SteamVR).
    pub voice_local_models: Option<bool>,
    /// Lara (the router on this PC, processor only) in front of an online brain (`voice_lara`, the Work first-run
    /// question; docs/WORK-FIRST-RUN.md). With a local model Lara is the two-stage router as before (`voice_local`);
    /// with an online provider that has a key (Claude with a key, any compatible API) Lara sorts each command and the
    /// online model acts with that category's tools only. Claude Code too: Lara goes first, the rest goes to Claude Code.
    pub voice_lara: Option<bool>,
    /// Speech recognition: Auto / Small (processor) / Large (graphics card) (`asr`, default auto).
    pub asr: Asr,
    /// Top-level keys this version does not know, raw values (kept on save).
    pub extra: BTreeMap<String, String>,
    /// `[use]` entries this version does not know (kept on save).
    pub use_extra: BTreeMap<String, String>,
    /// Sections this version does not know, with their raw `key = value` lines, in file order (kept on save).
    pub other_sections: Vec<(String, Vec<(String, String)>)>,
    /// What the last load noticed (an unsafe file mode, unknown lines).
    pub warnings: Vec<String>,
}

/// The built-in Claude CLI provider (see [`AiConfig::provider`]).
fn builtin_claude_cli() -> &'static Provider {
    static P: std::sync::OnceLock<Provider> = std::sync::OnceLock::new();
    P.get_or_init(Provider::claude_cli)
}

/// A provider name the file can hold.
pub fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 32 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Where the file is: `$XDG_CONFIG_HOME/omarchyvr/ai.toml`, else `$HOME/.config/omarchyvr/ai.toml`.
pub fn path_from(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let base = env("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).or_else(|| env("HOME").filter(|v| !v.is_empty()).map(|h| Path::new(&h).join(".config")))?;
    Some(base.join("omarchyvr/ai.toml"))
}

/// [`path_from`] with this process's environment.
pub fn default_path() -> Option<PathBuf> {
    path_from(&|k| std::env::var(k).ok())
}

fn unquote(v: &str, line: usize) -> Result<String, String> {
    let err = |w: &str| format!("line {line}: {w}");
    if let Some(r) = v.strip_prefix('\'') {
        let end = r.find('\'').ok_or_else(|| err("unterminated string"))?;
        let rest = r[end + 1..].trim();
        if !rest.is_empty() && !rest.starts_with('#') {
            return Err(err("text after the string"));
        }
        return Ok(r[..end].to_string());
    }
    let r = v.strip_prefix('"').ok_or_else(|| err("expected a quoted string"))?;
    let mut out = String::new();
    let mut it = r.chars();
    loop {
        match it.next() {
            None => return Err(err("unterminated string")),
            Some('"') => break,
            Some('\\') => match it.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('u') => {
                    let h: String = it.by_ref().take(4).collect();
                    let c = u32::from_str_radix(&h, 16).ok().and_then(char::from_u32).ok_or_else(|| err("bad \\u escape"))?;
                    out.push(c);
                }
                _ => return Err(err("bad escape")),
            },
            Some(c) => out.push(c),
        }
    }
    let rest: String = it.collect();
    let rest = rest.trim();
    if !rest.is_empty() && !rest.starts_with('#') {
        return Err(err("text after the string"));
    }
    Ok(out)
}

fn quote(v: &str) -> String {
    let mut o = String::from("\"");
    for c in v.chars() {
        match c {
            '"' => o += "\\\"",
            '\\' => o += "\\\\",
            '\n' => o += "\\n",
            '\t' => o += "\\t",
            '\r' => o += "\\r",
            c if c.is_control() => o += &format!("\\u{:04x}", c as u32),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

enum Section {
    Top,
    Use,
    Provider(usize),
    TwoStage,
    Other(usize),
}

fn parse_bool(v: &str, n: usize, k: &str) -> Result<bool, String> {
    match v.split('#').next().unwrap_or("").trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(format!("line {n}: {k} is true or false")),
    }
}

impl AiConfig {
    /// Parse the file text; errors name the line.
    pub fn parse(text: &str) -> Result<AiConfig, String> {
        let mut c = AiConfig::default();
        let mut kinds: Vec<bool> = Vec::new();
        let mut sec = Section::Top;
        for (i, raw) in text.lines().enumerate() {
            let n = i + 1;
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(h) = line.strip_prefix('[') {
                let h = h.split(']').next().unwrap_or("").trim();
                sec = if h == "use" {
                    Section::Use
                } else if h == "local_two_stage" {
                    c.local_two_stage.get_or_insert_with(LocalTwoStage::default);
                    Section::TwoStage
                } else if let Some(name) = h.strip_prefix("providers.") {
                    let name = name.trim().trim_matches('"');
                    if !valid_name(name) {
                        return Err(format!("line {n}: provider names are letters, digits, - and _"));
                    }
                    if c.providers.iter().any(|p| p.name == name) {
                        return Err(format!("line {n}: provider {name} twice"));
                    }
                    c.providers.push(Provider { name: name.to_string(), kind: Kind::OpenAiCompatible, model: String::new(), api_key: None, api_key_env: None, base_url: None, action_model: None, command: None, extra: BTreeMap::new() });
                    kinds.push(false);
                    Section::Provider(c.providers.len() - 1)
                } else {
                    // another tool's section: kept as it is
                    c.other_sections.push((h.to_string(), Vec::new()));
                    Section::Other(c.other_sections.len() - 1)
                };
                continue;
            }
            let (k, v) = line.split_once('=').ok_or_else(|| format!("line {n}: expected key = value"))?;
            let (k, v) = (k.trim(), v.trim());
            match &sec {
                Section::Top => match k {
                    "default" => c.default = Some(unquote(v, n)?).filter(|x| !x.is_empty()),
                    "voice_ai" => {
                        let m = unquote(v, n)?;
                        c.voice_ai = VoiceAi::parse(&m).ok_or_else(|| format!("line {n}: voice_ai {m:?} (off, fallback or always)"))?;
                    }
                    "strip_titles" => c.strip_titles = parse_bool(v, n, k)?,
                    "voice_online_backup" => c.voice_online_backup = Some(parse_bool(v, n, k)?),
                    "voice_local_models" => c.voice_local_models = Some(parse_bool(v, n, k)?),
                    "voice_lara" => c.voice_lara = Some(parse_bool(v, n, k)?),
                    "asr" => {
                        let m = unquote(v, n)?;
                        c.asr = Asr::parse(&m).ok_or_else(|| format!("line {n}: asr {m:?} (cpu or gpu)"))?;
                    }
                    "voice_local" => {
                        let m = unquote(v, n)?;
                        c.voice_local = VoiceLocal::parse(&m).ok_or_else(|| format!("line {n}: voice_local {m:?} (simple or two-stage)"))?;
                    }
                    "voice_confirm" => {
                        let m = unquote(v, n)?;
                        c.voice_confirm = VoiceConfirm::parse(&m).ok_or_else(|| format!("line {n}: voice_confirm {m:?} (none, dangerous or always)"))?;
                    }
                    "voice_listen" => {
                        let m = unquote(v, n)?;
                        c.voice_listen = VoiceListen::parse(&m).ok_or_else(|| format!("line {n}: voice_listen {m:?} (normal, long or very-long)"))?;
                    }
                    "voice_ai_gaze" => c.voice_ai_gaze = Some(parse_bool(v, n, k)?),
                    "voice_ai_log" => c.voice_ai_log = Some(parse_bool(v, n, k)?),
                    "voice_ai_prewarm" => c.voice_ai_prewarm = Some(parse_bool(v, n, k)?),
                    "voice_ai_timeout_s" => {
                        let t: f64 = v.split('#').next().unwrap_or("").trim().parse().map_err(|_| format!("line {n}: voice_ai_timeout_s is a number of seconds"))?;
                        if !(t.is_finite() && t > 0.0 && t <= 600.0) {
                            return Err(format!("line {n}: voice_ai_timeout_s is between 0 and 600 seconds"));
                        }
                        c.voice_ai_timeout_s = Some(t);
                    }
                    _ => {
                        // another tool's key: kept as it is
                        c.extra.insert(k.to_string(), v.to_string());
                    }
                },
                Section::Use => match Feature::ALL.into_iter().find(|f| f.id() == k) {
                    Some(f) => {
                        let p = unquote(v, n)?;
                        if !p.is_empty() {
                            c.uses.insert(f, p);
                        }
                    }
                    None => {
                        c.use_extra.insert(k.to_string(), v.to_string());
                    }
                },
                Section::Provider(ix) => {
                    let p = &mut c.providers[*ix];
                    match k {
                        "kind" => {
                            let s = unquote(v, n)?;
                            p.kind = Kind::parse(&s).ok_or_else(|| format!("line {n}: kind {s:?} (anthropic, openai, openai-compatible or claude-cli)"))?;
                            kinds[*ix] = true;
                        }
                        "model" => p.model = unquote(v, n)?,
                        "api_key" => p.api_key = Some(Secret::new(&unquote(v, n)?)).filter(|s| !s.is_empty()),
                        "api_key_env" => p.api_key_env = Some(unquote(v, n)?).filter(|s| !s.is_empty()),
                        "base_url" => p.base_url = Some(unquote(v, n)?).filter(|s| !s.is_empty()),
                        "action_model" => p.action_model = Some(unquote(v, n)?).filter(|s| !s.trim().is_empty()),
                        "command" => p.command = Some(unquote(v, n)?).filter(|s| !s.trim().is_empty()),
                        _ => {
                            p.extra.insert(k.to_string(), v.to_string());
                        }
                    }
                }
                Section::TwoStage => {
                    let t = c.local_two_stage.get_or_insert_with(LocalTwoStage::default);
                    match k {
                        "preset" => {
                            let m = unquote(v, n)?;
                            t.preset = if m == "custom" { None } else { Some(Preset::parse(&m).ok_or_else(|| format!("line {n}: preset {m:?} (low, medium, high or custom)"))?) };
                        }
                        "router" => {
                            let m = unquote(v, n)?;
                            if !ROUTER_MODELS.iter().any(|x| x.id == m) {
                                return Err(format!("line {n}: router {m:?} ({})", ROUTER_MODELS.map(|x| x.id).join(", ")));
                            }
                            t.router = m;
                        }
                        "action" => {
                            let m = unquote(v, n)?;
                            if !ACTION_MODELS.iter().any(|x| x.id == m) {
                                return Err(format!("line {n}: action {m:?} ({})", ACTION_MODELS.map(|x| x.id).join(", ")));
                            }
                            t.action = m;
                        }
                        "agent" => {
                            let m = unquote(v, n)?;
                            if m.is_empty() || m == "off" || m == "none" {
                                t.agent = None;
                            } else if AGENT_MODELS.iter().any(|x| x.id == m) {
                                t.agent = Some(m);
                            } else {
                                return Err(format!("line {n}: agent {m:?} ({}, or off)", AGENT_MODELS.map(|x| x.id).join(", ")));
                            }
                        }
                        "gpu_layers" => {
                            let raw = v.split('#').next().unwrap_or("").trim();
                            t.gpu_layers = if raw == "\"all\"" || raw == "'all'" {
                                None
                            } else {
                                Some(raw.parse().map_err(|_| format!("line {n}: gpu_layers is \"all\" or a number"))?)
                            };
                        }
                        "vram_mib" => {
                            let raw = v.split('#').next().unwrap_or("").trim();
                            t.vram_mib = if raw == "\"auto\"" || raw == "'auto'" {
                                None
                            } else {
                                let x: u32 = raw.parse().map_err(|_| format!("line {n}: vram_mib is \"auto\" or a number of MiB"))?;
                                if !(512..=65536).contains(&x) {
                                    return Err(format!("line {n}: vram_mib is between 512 and 65536"));
                                }
                                Some(x)
                            };
                        }
                        _ => c.warnings.push(format!("line {n}: [local_two_stage] {k} is not known (dropped on save)")),
                    }
                }
                Section::Other(ix) => c.other_sections[*ix].1.push((k.to_string(), v.to_string())),
            }
        }
        if let Some(p) = c.providers.iter().zip(&kinds).find(|(_, k)| !**k).map(|(p, _)| p) {
            return Err(format!("provider {} has no kind (anthropic, openai, openai-compatible or claude-cli)", p.name));
        }
        Ok(c)
    }

    /// The file text (keys included: only for [`AiConfig::save_to`]).
    pub fn render(&self) -> String {
        let mut o = String::from("# Pimax AI providers: written by the Pimax app (AI tab), read by voice control and Autofix.\n# This file holds API keys: keep it private (mode 600). api_key_env = \"NAME\" reads a key from the environment instead.\n");
        if let Some(d) = &self.default {
            o += &format!("default = {}\n", quote(d));
        }
        o += &format!("voice_ai = {}\nstrip_titles = {}\nvoice_confirm = {}\n", quote(self.voice_ai.id()), self.strip_titles, quote(self.voice_confirm.id()));
        o += &format!("voice_listen = {}\n", quote(self.voice_listen.id()));
        if let Some(t) = self.voice_ai_timeout_s {
            o += &format!("voice_ai_timeout_s = {t}\n");
        }
        if let Some(g) = self.voice_ai_gaze {
            o += &format!("voice_ai_gaze = {g}\n");
        }
        if let Some(l) = self.voice_ai_log {
            o += &format!("voice_ai_log = {l}\n");
        }
        if let Some(b) = self.voice_ai_prewarm {
            o += &format!("voice_ai_prewarm = {b}\n");
        }
        if let Some(b) = self.voice_online_backup {
            o += &format!("voice_online_backup = {b}\n");
        }
        if self.voice_local != VoiceLocal::Simple {
            o += &format!("voice_local = {}\n", quote(self.voice_local.id()));
        }
        if let Some(b) = self.voice_local_models {
            o += &format!("voice_local_models = {b}\n");
        }
        if let Some(b) = self.voice_lara {
            o += &format!("voice_lara = {b}\n");
        }
        if self.asr != Asr::Cpu {
            o += &format!("asr = {}\n", quote(self.asr.id()));
        }
        for (k, v) in &self.extra {
            o += &format!("{k} = {v}\n");
        }
        if !self.uses.is_empty() || !self.use_extra.is_empty() {
            o += "\n[use]\n";
            for (f, p) in &self.uses {
                o += &format!("{} = {}\n", f.id(), quote(p));
            }
            for (k, v) in &self.use_extra {
                o += &format!("{k} = {v}\n");
            }
        }
        for p in &self.providers {
            o += &format!("\n[providers.{}]\nkind = {}\n", p.name, quote(p.kind.id()));
            if !p.model.is_empty() {
                o += &format!("model = {}\n", quote(&p.model));
            }
            if let Some(k) = &p.api_key {
                o += &format!("api_key = {}\n", quote(k.expose()));
            }
            if let Some(e) = &p.api_key_env {
                o += &format!("api_key_env = {}\n", quote(e));
            }
            if let Some(b) = &p.base_url {
                o += &format!("base_url = {}\n", quote(b));
            }
            if let Some(m) = &p.action_model {
                o += &format!("action_model = {}\n", quote(m));
            }
            if let Some(c) = &p.command {
                o += &format!("command = {}\n", quote(c));
            }
            for (k, v) in &p.extra {
                o += &format!("{k} = {v}\n");
            }
        }
        if let Some(t) = &self.local_two_stage {
            o += &format!("\n[local_two_stage]\npreset = {}\nrouter = {}\naction = {}\n", quote(t.preset.map(|p| p.id()).unwrap_or("custom")), quote(&t.router), quote(&t.action));
            if let Some(a) = &t.agent {
                o += &format!("agent = {}\n", quote(a));
            }
            o += &match t.gpu_layers {
                None => "gpu_layers = \"all\"\n".to_string(),
                Some(n) => format!("gpu_layers = {n}\n"),
            };
            o += &match t.vram_mib {
                None => "vram_mib = \"auto\"\n".to_string(),
                Some(m) => format!("vram_mib = {m}\n"),
            };
        }
        for (name, lines) in &self.other_sections {
            o += &format!("\n[{name}]\n");
            for (k, v) in lines {
                o += &format!("{k} = {v}\n");
            }
        }
        o
    }

    /// Load from the default path ([`default_path`]); a missing file is an empty config.
    pub fn load() -> Result<AiConfig, String> {
        let p = default_path().ok_or("neither XDG_CONFIG_HOME nor HOME is set")?;
        AiConfig::load_from(&p)
    }

    /// Load a file; a missing file is an empty config. Warns when group or others may read it.
    pub fn load_from(path: &Path) -> Result<AiConfig, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(AiConfig::default()),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let mut c = AiConfig::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if let Some(w) = mode_warning(path) {
            c.warnings.insert(0, w);
        }
        Ok(c)
    }

    /// Save to the default path.
    pub fn save(&self) -> Result<(), String> {
        let p = default_path().ok_or("neither XDG_CONFIG_HOME nor HOME is set")?;
        self.save_to(&p)
    }

    /// Write atomically (a new file with mode 0600, fsync, rename, fsync the directory). A directory created here gets
    /// mode 0700.
    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
        let dir = path.parent().unwrap_or(Path::new("."));
        if !dir.exists() {
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let tmp = dir.join(format!(".ai.toml.tmp{}", std::process::id()));
        let _ = std::fs::remove_file(&tmp);
        let res = (|| -> std::io::Result<()> {
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
            f.write_all(self.render().as_bytes())?;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            f.sync_all()?;
            std::fs::rename(&tmp, path)?;
            if let Ok(d) = std::fs::File::open(dir) {
                let _ = d.sync_all();
            }
            Ok(())
        })();
        if res.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        res.map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The first provider on this PC (loopback base URL), by name.
    pub fn local_provider(&self) -> Option<String> {
        self.providers.iter().find(|p| p.base().is_some_and(is_loopback)).map(|p| p.name.clone())
    }

    /// The first online provider (OpenRouter first, then a keyed one, then the Claude CLI), by name. The built-in
    /// Claude CLI provider always exists, so there is always one.
    pub fn online_provider(&self) -> Option<String> {
        let on = || self.providers.iter().filter(|p| p.kind != Kind::ClaudeCli && p.base().is_some_and(|b| !is_loopback(b)));
        on().find(|p| p.base().is_some_and(|b| b.contains("openrouter")))
            .or_else(|| on().next())
            .map(|p| p.name.clone())
            .or_else(|| Some(self.claude_cli_provider().name.clone()))
    }

    /// The Claude CLI provider voice uses by default: the first one in the file, else the built-in one.
    pub fn claude_cli_provider(&self) -> &Provider {
        self.providers.iter().find(|p| p.kind == Kind::ClaudeCli).unwrap_or_else(|| builtin_claude_cli())
    }

    /// Where voice commands are understood now.
    pub fn runs_on(&self) -> RunsOn {
        if self.provider_for(Feature::Voice).is_some_and(|p| p.is_remote()) {
            RunsOn::Online
        } else if self.voice_local == VoiceLocal::TwoStage {
            RunsOn::TwoStage
        } else {
            RunsOn::Simple
        }
    }

    /// The choices this file allows (Online only with an online provider).
    pub fn runs_on_choices(&self) -> Vec<RunsOn> {
        let mut v = vec![RunsOn::Simple, RunsOn::TwoStage];
        if self.online_provider().is_some() {
            v.push(RunsOn::Online);
        }
        v
    }

    /// The agent-mode model option (docs/AI.md section 18): `Some(id)` sets `[local_two_stage] agent` and the top-level
    /// `voice_agent_url` / `voice_agent_model` that voice reads (goal steps only; the usual model is the fallback while
    /// the 27B server is not up); `None` removes all three. Nothing else in the file changes.
    pub fn set_agent(&mut self, id: Option<&str>) -> Result<(), String> {
        match id {
            Some(id) => {
                if !AGENT_MODELS.iter().any(|m| m.id == id) {
                    return Err(format!("{id}: not an agent model ({})", AGENT_MODELS.map(|x| x.id).join(", ")));
                }
                self.local_two_stage.get_or_insert_with(LocalTwoStage::default).agent = Some(id.into());
                self.extra.insert("voice_agent_url".into(), format!("\"http://127.0.0.1:{AGENT_PORT}/v1/chat/completions\""));
                self.extra.insert("voice_agent_model".into(), "\"pimax-local\"".into());
            }
            None => {
                if let Some(t) = self.local_two_stage.as_mut() {
                    t.agent = None;
                }
                self.extra.remove("voice_agent_url");
                self.extra.remove("voice_agent_model");
            }
        }
        Ok(())
    }

    /// Switch: `[use] voice` and `voice_local` only (keys and other providers stay as they are). A local choice adds
    /// the `omarchyvr-ai` provider (127.0.0.1:8080) when there is no local one.
    pub fn set_runs_on(&mut self, r: RunsOn) -> Result<(), String> {
        let name = match r {
            RunsOn::Online => self.online_provider().ok_or("no online provider is set up (AI page: Add OpenRouter)")?,
            RunsOn::Simple | RunsOn::TwoStage => {
                self.voice_local = if r == RunsOn::TwoStage { VoiceLocal::TwoStage } else { VoiceLocal::Simple };
                if r == RunsOn::TwoStage {
                    self.local_two_stage.get_or_insert_with(LocalTwoStage::default);
                }
                match self.local_provider() {
                    Some(n) => n,
                    None => {
                        let mut name = "local".to_string();
                        let mut i = 2;
                        while self.provider(&name).is_some() {
                            name = format!("local-{i}");
                            i += 1;
                        }
                        let mut p = Provider::new(&name, Kind::OpenAiCompatible);
                        p.base_url = Some(LLAMA_SERVER_BASE.into());
                        p.model = "pimax-local".into();
                        self.providers.push(p);
                        name
                    }
                }
            }
        };
        self.uses.insert(Feature::Voice, name);
        Ok(())
    }

    /// Local models on (the switch; not set: on when voice uses a provider on this PC).
    pub fn local_models_on(&self) -> bool {
        self.voice_local_models.unwrap_or_else(|| self.runs_on() != RunsOn::Online)
    }

    /// Lara sorts the commands for an online brain (`voice_lara = true`, and voice uses an online provider: Claude with
    /// an API key, any compatible API, or Claude Code). Lara goes first; what it does not handle goes to the brain, and
    /// without Lara installed everything does.
    pub fn lara_with_online(&self) -> bool {
        self.voice_lara == Some(true) && self.provider_for(Feature::Voice).is_some_and(|p| p.is_remote())
    }

    /// Lara is in use: two stages on this PC, or in front of an online brain.
    pub fn lara_on(&self) -> bool {
        (self.local_models_on() && self.voice_local == VoiceLocal::TwoStage) || self.lara_with_online()
    }

    /// The local AI servers have work in a Work session: the local models, or Lara alone in front of an online brain.
    /// Only Work starts them; Play never does (docs/PLAY-WORK-AUDIT.md).
    pub fn ai_servers_wanted(&self) -> bool {
        self.local_models_on() || self.lara_with_online()
    }

    /// "Local models: On / Off". On: two stages with the preset (Medium unless one was chosen), voice on this PC.
    /// Off: the online provider when `online`, else voice keeps the local provider with no model loaded (only the
    /// offline commands answer). Keys stay as they are.
    pub fn set_local_models(&mut self, on: bool, online: bool) -> Result<(), String> {
        self.voice_local_models = Some(on);
        if on {
            let t = self.local_two_stage.get_or_insert_with(LocalTwoStage::default);
            if t.preset.is_none() && t.matching().is_none() && t.router.is_empty() {
                t.apply(Preset::Medium);
            }
            self.set_runs_on(RunsOn::TwoStage)
        } else if online {
            self.set_runs_on(RunsOn::Online)
        } else {
            if self.runs_on() == RunsOn::Online {
                self.set_runs_on(RunsOn::TwoStage)?;
            }
            Ok(())
        }
    }

    /// The preset of the local models (Medium when none was set; None: chosen by hand).
    pub fn preset(&self) -> Option<Preset> {
        let t = self.two_stage();
        t.preset.or_else(|| t.matching())
    }

    /// Choose a preset (the models of it; the VRAM limit stays).
    pub fn set_preset(&mut self, p: Preset) {
        self.local_two_stage.get_or_insert_with(LocalTwoStage::default).apply(p);
    }

    /// The next choice (the watch button cycles).
    pub fn next_runs_on(&self) -> RunsOn {
        let c = self.runs_on_choices();
        let i = c.iter().position(|x| *x == self.runs_on()).unwrap_or(0);
        c[(i + 1) % c.len()]
    }

    /// The two-stage settings (the defaults when the section is missing).
    pub fn two_stage(&self) -> LocalTwoStage {
        self.local_two_stage.clone().unwrap_or_default()
    }

    /// A provider by name (the built-in `claude-cli` exists even when the file has no such table).
    pub fn provider(&self, name: &str) -> Option<&Provider> {
        self.providers.iter().find(|p| p.name == name).or_else(|| (name == CLAUDE_CLI_NAME).then(builtin_claude_cli))
    }

    /// The provider a feature uses: its `[use]` entry, else `default`; then, for voice control, the Claude CLI
    /// (no key needed: the default since 2026-10-16); for the other features the only provider.
    pub fn provider_for(&self, f: Feature) -> Option<&Provider> {
        self.uses
            .get(&f)
            .and_then(|n| self.provider(n))
            .or_else(|| self.default.as_deref().and_then(|n| self.provider(n)))
            .or_else(|| match self.providers.len() {
                // one provider in the file: that one (the rule from before the Claude CLI existed)
                1 => self.providers.first(),
                // a new install: nothing configured, so voice uses the built-in Claude CLI
                0 if f == Feature::Voice => Some(self.claude_cli_provider()),
                _ => None,
            })
    }

    /// What a feature talks to, with this process's environment.
    pub fn resolve(&self, f: Feature) -> Result<Resolved, String> {
        self.resolve_with(f, &|k| std::env::var(k).ok(), None)
    }

    /// What a feature talks to. `session_key`: a key the caller keeps for this session only (used when the file and
    /// the environment have none).
    pub fn resolve_with(&self, f: Feature, env: &dyn Fn(&str) -> Option<String>, session_key: Option<&Secret>) -> Result<Resolved, String> {
        let p = self.provider_for(f).ok_or_else(|| format!("no AI provider for {} (AI tab: add one and pick it)", f.label()))?;
        self.resolve_provider(&p.name, env, session_key)
    }

    /// One provider by name, as [`AiConfig::resolve_with`] would give it.
    pub fn resolve_provider(&self, name: &str, env: &dyn Fn(&str) -> Option<String>, session_key: Option<&Secret>) -> Result<Resolved, String> {
        let p = self.provider(name).ok_or_else(|| format!("no provider named {name}"))?;
        if p.kind == Kind::ClaudeCli {
            let model = Some(p.model.trim()).filter(|m| !m.is_empty()).unwrap_or(CLAUDE_CLI_MODELS[0].0).to_string();
            let command = p.command.clone().filter(|c| !c.trim().is_empty());
            return Ok(Resolved { provider: p.name.clone(), kind: p.kind, model, base_url: String::new(), api_key: None, key_source: KeySource::None, action_model: None, command });
        }
        let base_url = p.base().map(|b| b.trim_end_matches('/').to_string()).ok_or_else(|| format!("{}: base_url is missing", p.name))?;
        let model = if p.model.trim().is_empty() {
            if p.kind == Kind::Anthropic {
                CLAUDE_MODELS[0].0.to_string()
            } else {
                return Err(format!("{}: no model set", p.name));
            }
        } else {
            p.model.trim().to_string()
        };
        let env_name = p.api_key_env.clone().or_else(|| p.kind.usual_env().map(String::from));
        let (api_key, key_source) = if let Some(k) = &p.api_key {
            (Some(k.clone()), KeySource::File)
        } else if let Some(k) = env_name.as_deref().and_then(|n| env(n).filter(|v| !v.trim().is_empty()).map(|v| (n.to_string(), v))) {
            (Some(Secret::new(&k.1)), KeySource::Env(k.0))
        } else if let Some(k) = session_key.filter(|k| !k.is_empty()) {
            (Some(k.clone()), KeySource::Session)
        } else if p.kind.needs_key() {
            return Err(format!("{}: no API key (AI tab: add the key, or set api_key_env)", p.name));
        } else {
            (None, KeySource::None)
        };
        Ok(Resolved { provider: p.name.clone(), kind: p.kind, model, base_url, api_key, key_source, action_model: p.action_model.clone(), command: None })
    }

    /// Text for logs and reports: everything except the keys.
    pub fn redacted(&self) -> Redacted<'_> {
        Redacted(self)
    }
}

/// [`AiConfig::redacted`]: shows whether a key is set, never any character of it.
pub struct Redacted<'a>(&'a AiConfig);

impl fmt::Display for Redacted<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let c = self.0;
        writeln!(f, "default = {}", c.default.as_deref().unwrap_or("(none)"))?;
        writeln!(f, "voice_ai = {}, strip_titles = {}", c.voice_ai.id(), c.strip_titles)?;
        for (feat, p) in &c.uses {
            writeln!(f, "use.{} = {p}", feat.id())?;
        }
        for p in &c.providers {
            let key = match (&p.api_key, &p.api_key_env) {
                (Some(_), _) => "key in the file".to_string(),
                (None, Some(e)) => format!("key from ${e}"),
                (None, None) if p.kind == Kind::ClaudeCli => "the Claude CLI's own login".to_string(),
                (None, None) if p.kind.needs_key() => "no key".to_string(),
                (None, None) => "no key needed".to_string(),
            };
            writeln!(f, "[{}] {} model={} base={} ({key})", p.name, p.kind.id(), if p.model.is_empty() { "-" } else { &p.model }, p.base().unwrap_or("-"))?;
        }
        Ok(())
    }
}

/// A warning when group or others may read the file.
pub fn mode_warning(path: &Path) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    let m = std::fs::metadata(path).ok()?.permissions().mode() & 0o777;
    (m & 0o077 != 0).then(|| format!("{} can be read by others (mode {m:o}): it holds API keys, run chmod 600 on it", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_home(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("omarchyvr-ai-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn sample() -> AiConfig {
        let mut claude = Provider::new("claude", Kind::Anthropic);
        claude.api_key = Some(Secret::new("sk-ant-api03-TEST-NOT-A-KEY-123"));
        let mut openai = Provider::new("openai", Kind::OpenAi);
        openai.model = "gpt-test".into();
        openai.api_key_env = Some("MY_OPENAI".into());
        let mut local = Provider::new("local", Kind::OpenAiCompatible);
        local.model = "qwen3:8b".into();
        local.action_model = Some("other-model".into());
        AiConfig {
            default: Some("claude".into()),
            uses: BTreeMap::from([(Feature::Autofix, "local".to_string())]),
            providers: vec![claude, openai, local],
            voice_ai: VoiceAi::Always,
            strip_titles: true,
            ..Default::default()
        }
    }

    #[test]
    fn cloud_ai_is_off_by_default() {
        // no file, or a file without providers: no keyed provider, no default provider; voice uses the built-in Claude
        // CLI (the user's own login, no key); the other features have nothing to talk to
        for c in [AiConfig::default(), AiConfig::parse("").unwrap(), AiConfig::parse("strip_titles = true\n").unwrap()] {
            assert!(c.providers.is_empty() && c.default.is_none() && c.uses.is_empty());
            let r = c.resolve_with(Feature::Voice, &|_| None, None).unwrap();
            assert_eq!((r.kind, r.model.as_str(), r.provider.as_str(), r.api_key.is_none(), r.local()), (Kind::ClaudeCli, "haiku", "claude-cli", true, false));
            assert_eq!(c.voice_ai, VoiceAi::Fallback);
            assert!(c.resolve_with(Feature::Autofix, &|_| None, None).is_err(), "nothing to send to without a key");
        }
        // a provider the user adds (a key) is kept
        let c = AiConfig::parse("default = \"claude\"\n[providers.claude]\nkind = \"anthropic\"\napi_key = \"sk-ant-x\"\n").unwrap();
        assert_eq!(c.providers.len(), 1);
    }

    #[test]
    fn asr_setting_parses_renders_and_defaults_to_the_processor() {
        assert_eq!(AiConfig::default().asr, Asr::Cpu);
        assert!(!AiConfig::default().asr_on_gpu());
        assert!(!AiConfig::default().render().contains("asr"), "the processor is the absent key");
        let c = AiConfig::parse("asr = \"gpu\"\n").unwrap();
        assert_eq!(c.asr, Asr::Gpu);
        assert!(c.asr_on_gpu());
        assert_eq!(AiConfig::parse(&c.render()).unwrap().asr, Asr::Gpu);
        assert_eq!(AiConfig::parse("asr = 'cpu' # small\n").unwrap().asr, Asr::Cpu);
        // migration: the old default (no key, or an explicit "auto") is the processor now
        assert_eq!(AiConfig::parse("").unwrap().asr, Asr::Cpu);
        assert_eq!(AiConfig::parse("asr = \"auto\"\n").unwrap().asr, Asr::Cpu);
        let e = AiConfig::parse("asr = \"huge\"\n").unwrap_err();
        assert!(e.contains("line 1") && e.contains("cpu or gpu"), "{e}");
    }

    #[test]
    fn asr_plan_fits_the_graphics_card() {
        // the processor never uses the card
        assert!(!asr_plan(Asr::Cpu, true, Some(20000)).gpu);
        assert!(asr_plan(Asr::Cpu, true, Some(20000)).why.contains("processor"));
        // the card needs room and the local models
        assert!(asr_plan(Asr::Gpu, true, Some(2000)).gpu);
        assert!(asr_plan(Asr::Gpu, true, None).gpu);
        let p = asr_plan(Asr::Gpu, true, Some(800));
        assert!(!p.gpu && p.why.contains("Not enough video memory"), "{p:?}");
        let p = asr_plan(Asr::Gpu, false, Some(8000));
        assert!(!p.gpu && p.why.contains("Local models On"), "{p:?}");
    }

    #[test]
    fn round_trip_and_resolve() {
        let c = sample();
        let text = c.render();
        assert_eq!(AiConfig::parse(&text).unwrap(), c);
        let env = |k: &str| (k == "MY_OPENAI").then(|| "sk-openai-env".to_string());
        // voice: the default (Claude, key in the file)
        let v = c.resolve_with(Feature::Voice, &env, None).unwrap();
        assert_eq!((v.provider.as_str(), v.kind, v.model.as_str()), ("claude", Kind::Anthropic, "claude-haiku-5-5"));
        assert_eq!(v.endpoint(), "https://api.anthropic.com/v1/messages");
        assert_eq!(v.key_source, KeySource::File);
        assert!(!v.local());
        // autofix: the local server, no key
        let a = c.resolve_with(Feature::Autofix, &env, None).unwrap();
        assert_eq!(a.endpoint(), "http://127.0.0.1:11434/v1/chat/completions");
        assert!(a.api_key.is_none() && a.local());
        // a key from the environment
        let mut c2 = c.clone();
        c2.default = Some("openai".into());
        let o = c2.resolve_with(Feature::Voice, &env, None).unwrap();
        assert_eq!((o.key_source.clone(), o.api_key.as_ref().map(|k| k.expose().to_string())), (KeySource::Env("MY_OPENAI".into()), Some("sk-openai-env".into())));
        assert_eq!(o.endpoint(), "https://api.openai.com/v1/chat/completions");
        // no key anywhere: an error, unless the caller has a session key
        let none = |_: &str| None;
        assert!(c2.resolve_with(Feature::Voice, &none, None).unwrap_err().contains("no API key"));
        let s = Secret::new("session-key-xyz");
        assert_eq!(c2.resolve_with(Feature::Voice, &none, Some(&s)).unwrap().key_source, KeySource::Session);
        // no provider at all: voice has the Claude CLI, autofix has nothing
        assert_eq!(AiConfig::default().resolve_with(Feature::Voice, &none, None).unwrap().kind, Kind::ClaudeCli);
        assert!(AiConfig::default().resolve_with(Feature::Autofix, &none, None).is_err());
    }

    #[test]
    fn unknown_keys_and_the_optional_voice_settings_survive_a_save() {
        let t = "# by hand\nvoice_ai = \"always\"\nvoice_ai_timeout_s = 2.5\nvoice_ai_gaze = false\nvoice_ai_log = true\nnew_feature = \"x\" # a later tool\nlimits = [1, 2]\ndefault = \"local\"\n\n[use]\nvoice = \"local\"\nwindows = \"local\"\n\n[providers.local]\nkind = \"openai-compatible\"\nbase_url = \"http://127.0.0.1:8080/v1\"\nmodel = \"pimax-local\"\nctx = 8192\n\n[llm]\nurl = \"http://127.0.0.1:8080\"\ntimeout = 3\n";
        let c = AiConfig::parse(t).unwrap();
        assert_eq!((c.voice_ai, c.voice_ai_timeout_s, c.voice_ai_gaze, c.voice_ai_log), (VoiceAi::Always, Some(2.5), Some(false), Some(true)));
        assert_eq!(c.extra.get("new_feature").map(String::as_str), Some("\"x\" # a later tool"));
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        let back = AiConfig::parse(&c.render()).unwrap();
        assert_eq!(back, c, "nothing is lost on a save");
        let r = c.render();
        for l in ["voice_ai_timeout_s = 2.5", "voice_ai_gaze = false", "voice_ai_log = true", "limits = [1, 2]", "windows = \"local\"", "ctx = 8192", "[llm]", "timeout = 3"] {
            assert!(r.contains(l), "{l} missing in\n{r}");
        }
        // top-level keys come before every section
        assert!(r.find("new_feature").unwrap() < r.find("[use]").unwrap());
        assert!(AiConfig::parse("voice_ai_timeout_s = -1\n").is_err() && AiConfig::parse("voice_ai_gaze = maybe\n").is_err());
        let home = tmp_home("keep");
        let p = home.join("ai.toml");
        c.save_to(&p).unwrap();
        assert_eq!(AiConfig::load_from(&p).unwrap().other_sections, c.other_sections);
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn agent_model_is_an_option_and_round_trips() {
        assert_eq!(AiConfig::parse("[local_two_stage]\naction = \"qwen3.5-4b\"\n").unwrap().two_stage().agent, None);
        let c = AiConfig::parse("voice_local = \"two-stage\"\n[local_two_stage]\nagent = \"qwen3.6-27b\"\n").unwrap();
        assert_eq!(c.two_stage().agent.as_deref(), Some("qwen3.6-27b"));
        assert_eq!(AiConfig::parse(&c.render()).unwrap(), c);
        assert_eq!(AiConfig::parse("[local_two_stage]\nagent = \"off\"\n").unwrap().two_stage().agent, None);
        assert!(AiConfig::parse("[local_two_stage]\nagent = \"gpt\"\n").unwrap_err().contains("agent"));
        assert!(local_model("qwen3.6-27b").is_some());
        let mut c = AiConfig::parse("voice_ai = \"fallback\"\n").unwrap();
        c.set_agent(Some("qwen3.6-27b")).unwrap();
        let back = AiConfig::parse(&c.render()).unwrap();
        assert_eq!(back.two_stage().agent.as_deref(), Some("qwen3.6-27b"));
        assert!(c.render().contains("voice_agent_url = \"http://127.0.0.1:8083/v1/chat/completions\"") && c.render().contains("voice_ai = "));
        c.set_agent(None).unwrap();
        assert!(!c.render().contains("voice_agent") && c.set_agent(Some("x")).is_err());
    }

    #[test]
    fn local_two_stage_round_trip_keeps_the_keys() {
        let t = "voice_local = \"two-stage\"\n[providers.or]\nkind = \"openai-compatible\"\nbase_url = \"https://openrouter.ai/api/v1\"\napi_key = \"sk-or-TEST\"\n\n[local_two_stage]\nrouter = \"qwen3.5-0.8b\"\naction = \"qwen3.5-9b\"\ngpu_layers = 20\nvram_mib = 4096\n";
        let c = AiConfig::parse(t).unwrap();
        assert_eq!(c.voice_local, VoiceLocal::TwoStage);
        let ts = c.two_stage();
        assert_eq!((ts.router.as_str(), ts.action.as_str(), ts.gpu_layers, ts.vram_mib), ("qwen3.5-0.8b", "qwen3.5-9b", Some(20), Some(4096)));
        let back = AiConfig::parse(&c.render()).unwrap();
        assert_eq!(back, c);
        assert_eq!(back.providers[0].api_key.as_ref().unwrap().expose(), "sk-or-TEST", "the key survives a mode change");
        // the defaults and their rendering
        let d = AiConfig::parse("").unwrap();
        assert_eq!((d.voice_local, d.two_stage()), (VoiceLocal::Simple, LocalTwoStage::default()));
        assert!(!d.render().contains("voice_local") && !d.render().contains("[local_two_stage]"), "simple files stay as they were");
        let mut all = d.clone();
        all.local_two_stage = Some(LocalTwoStage::default());
        assert!(all.render().contains("gpu_layers = \"all\"") && all.render().contains("vram_mib = \"auto\""), "Auto by default");
        assert_eq!(AiConfig::parse(&all.render()).unwrap().two_stage().vram_mib, None);
        assert_eq!(AiConfig::parse(&all.render()).unwrap().two_stage().gpu_layers, None);
        // unknown models and bad values are refused with the line
        assert!(AiConfig::parse("[local_two_stage]\nrouter = \"gpt\"\n").unwrap_err().contains("router"));
        assert!(AiConfig::parse("[local_two_stage]\nvram_mib = 9\n").is_err());
        assert!(AiConfig::parse("voice_local = \"three\"\n").is_err());
        assert!(local_model("laya").is_some_and(|m| m.engine == LocalEngine::Laya) && local_model("qwen3.5-9b").is_some());
    }

    #[test]
    fn local_models_switch_and_presets() {
        let t = "[use]\nvoice = \"local\"\n[providers.local]\nkind = \"openai-compatible\"\nbase_url = \"http://127.0.0.1:8080/v1\"\n[providers.or]\nkind = \"openai-compatible\"\nbase_url = \"https://openrouter.ai/api/v1\"\napi_key = \"sk-or-v1-TEST\"\n";
        let mut c = AiConfig::parse(t).unwrap();
        assert!(c.local_models_on(), "not set: follows [use] voice");
        c.set_local_models(true, false).unwrap();
        assert_eq!((c.runs_on(), c.preset()), (RunsOn::TwoStage, Some(Preset::Medium)));
        c.set_preset(Preset::Low);
        assert_eq!((c.two_stage().gpu_layers, c.two_stage().action.as_str()), (Some(0), "qwen3.5-4b"));
        c.set_preset(Preset::High);
        assert_eq!((c.two_stage().gpu_layers, c.two_stage().action.as_str()), (None, "qwen3.5-9b"));
        let back = AiConfig::parse(&c.render()).unwrap();
        assert_eq!((back.preset(), back.voice_local_models), (Some(Preset::High), Some(true)));
        assert!(back.render().contains("preset = \"high\""));
        // off with Online: Jev; off without: no model, the provider stays local
        c.set_local_models(false, true).unwrap();
        assert_eq!((c.runs_on(), c.local_models_on()), (RunsOn::Online, false));
        c.set_local_models(false, false).unwrap();
        assert_eq!((c.runs_on(), c.local_models_on()), (RunsOn::TwoStage, false));
        assert!(c.render().contains("voice_local_models = false") && c.render().contains("sk-or-v1-TEST"));
        // by hand: custom
        let mut t2 = c.two_stage();
        t2.router = "laya".into();
        t2.preset = None;
        c.local_two_stage = Some(t2);
        assert_eq!(c.preset(), None);
        assert!(AiConfig::parse(&c.render()).unwrap().two_stage().preset.is_none());
        assert_eq!(Preset::High.next(), Preset::Low);
    }

    #[test]
    fn runs_on_switches_only_use_and_voice_local() {
        let t = "default = \"local\"\n[use]\nvoice = \"local\"\n[providers.local]\nkind = \"openai-compatible\"\nbase_url = \"http://127.0.0.1:8080/v1\"\nmodel = \"pimax-local\"\n[providers.openrouter]\nkind = \"openai-compatible\"\nbase_url = \"https://openrouter.ai/api/v1\"\nmodel = \"typesafe/jev-router\"\napi_key = \"sk-or-v1-TEST\"\n";
        let mut c = AiConfig::parse(t).unwrap();
        assert_eq!(c.runs_on(), RunsOn::Simple);
        assert_eq!(c.runs_on_choices(), RunsOn::ALL.to_vec());
        assert_eq!(c.next_runs_on(), RunsOn::TwoStage);
        c.set_runs_on(RunsOn::TwoStage).unwrap();
        assert_eq!((c.runs_on(), c.next_runs_on()), (RunsOn::TwoStage, RunsOn::Online));
        c.set_runs_on(RunsOn::Online).unwrap();
        assert_eq!((c.runs_on(), c.next_runs_on()), (RunsOn::Online, RunsOn::Simple));
        let back = AiConfig::parse(&c.render()).unwrap();
        assert_eq!(back.providers[1].api_key.as_ref().unwrap().expose(), "sk-or-v1-TEST", "keys stay");
        assert_eq!(back.runs_on(), RunsOn::Online);
        // nothing set: the built-in Claude CLI is the online choice (Online, no key); no local provider: one is added
        let mut e = AiConfig::default();
        assert_eq!((e.runs_on(), e.runs_on_choices()), (RunsOn::Online, RunsOn::ALL.to_vec()));
        e.set_runs_on(RunsOn::Online).unwrap();
        assert_eq!(e.uses.get(&Feature::Voice).map(String::as_str), Some("claude-cli"));
        let mut e = AiConfig::default();
        e.set_runs_on(RunsOn::TwoStage).unwrap();
        assert_eq!(e.providers[0].base(), Some(LLAMA_SERVER_BASE));
        assert_eq!((e.runs_on(), e.uses.get(&Feature::Voice).map(String::as_str)), (RunsOn::TwoStage, Some("local")));
        assert_eq!(RunsOn::parse("lara"), Some(RunsOn::TwoStage));
    }

    #[test]
    fn claude_cli_is_the_voice_default_and_round_trips() {
        // a provider table of its own: model and command kept, no key written
        let t = "[providers.claude-cli]\nkind = \"claude-cli\"\nmodel = \"sonnet\"\ncommand = \"~/bin/claude\"\n";
        let c = AiConfig::parse(t).unwrap();
        let r = c.resolve_with(Feature::Voice, &|_| None, None).unwrap();
        assert_eq!((r.kind, r.model.as_str(), r.command.as_deref(), r.endpoint().as_str()), (Kind::ClaudeCli, "sonnet", Some("~/bin/claude"), "claude-cli"));
        assert_eq!(c.runs_on(), RunsOn::Online);
        assert!(c.provider_for(Feature::Voice).unwrap().is_remote());
        let back = AiConfig::parse(&c.render()).unwrap();
        assert_eq!(back.providers[0].command.as_deref(), Some("~/bin/claude"));
        assert!(!c.render().contains("api_key = "));
        // a file that only has a keyed provider and no choice: that one is used by both (the old rule)
        let t = "[providers.claude]\nkind = \"anthropic\"\napi_key = \"sk-ant-x\"\n";
        let c = AiConfig::parse(t).unwrap();
        assert_eq!(c.provider_for(Feature::Voice).unwrap().name, "claude");
        // a lone local provider stays the voice brain (an upgrade must not switch it to the cloud)
        let c = AiConfig::parse("[providers.local]\nkind = \"openai-compatible\"\nbase_url = \"http://127.0.0.1:8080/v1\"\n").unwrap();
        assert_eq!(c.provider_for(Feature::Voice).unwrap().name, "local");
        assert_eq!(c.runs_on(), RunsOn::Simple);
        // a new install (no providers): the built-in Claude CLI answers voice, autofix has nothing
        let c = AiConfig::parse("").unwrap();
        assert_eq!(c.provider_for(Feature::Voice).unwrap().kind, Kind::ClaudeCli);
        assert!(c.provider_for(Feature::Autofix).is_none());
        // two providers and no choice: nothing is guessed
        let two = "[providers.a]\nkind = \"openai-compatible\"\nbase_url = \"http://127.0.0.1:1/v1\"\n[providers.b]\nkind = \"openai-compatible\"\nbase_url = \"http://127.0.0.1:2/v1\"\n";
        assert!(AiConfig::parse(two).unwrap().provider_for(Feature::Voice).is_none());
        assert_eq!(AiConfig::parse(&format!("[use]\nvoice = \"b\"\n{two}")).unwrap().provider_for(Feature::Voice).unwrap().name, "b");
        assert_eq!(AiConfig::parse(&format!("default = \"a\"\n{two}")).unwrap().provider_for(Feature::Voice).unwrap().name, "a");
        let c = AiConfig::parse(t).unwrap();
        assert_eq!(c.provider_for(Feature::Autofix).unwrap().name, "claude");
        assert_eq!(c.online_provider().as_deref(), Some("claude"), "a keyed online provider is still found first");
        // an explicit choice is kept
        let c = AiConfig::parse(&format!("default = \"claude\"\n{t}")).unwrap();
        assert_eq!(c.provider_for(Feature::Voice).unwrap().name, "claude");
        let c = AiConfig::parse(&format!("[use]\nvoice = \"claude\"\n{t}")).unwrap();
        assert_eq!(c.resolve_with(Feature::Voice, &|_| None, None).unwrap().kind, Kind::Anthropic);
        // no model: haiku; the command word defaults to claude
        let c = AiConfig::parse("[providers.x]\nkind = \"claude-cli\"\n").unwrap();
        assert_eq!((c.providers[0].cli_command(), c.resolve_provider("x", &|_| None, None).unwrap().model.as_str()), ("claude", "haiku"));
        assert!(AiConfig::parse("[providers.x]\nkind = \"claude-cli\"\nbase_url = \"http://127.0.0.1:1/v1\"\n").is_ok());
    }

    #[test]
    fn keys_never_show() {
        let c = sample();
        let shown = format!("{} {:?} {}", c.redacted(), c, c.providers[0].api_key.as_ref().unwrap());
        assert!(!shown.contains("SECRET"), "{shown}");
        assert!(shown.contains("key in the file") && shown.contains("key from $MY_OPENAI") && shown.contains("no key needed"));
        assert!(!format!("{:?}", c.resolve_with(Feature::Voice, &|_| None, None)).contains("SECRET"));
    }

    #[test]
    fn save_is_private_and_load_warns_about_open_files() {
        use std::os::unix::fs::PermissionsExt;
        let home = tmp_home("save");
        let env = |k: &str| (k == "HOME").then(|| home.display().to_string());
        let path = path_from(&env).unwrap();
        assert_eq!(path, home.join(".config/omarchyvr/ai.toml"));
        assert_eq!(AiConfig::load_from(&path).unwrap(), AiConfig::default(), "a missing file is empty");
        let c = sample();
        c.save_to(&path).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        let back = AiConfig::load_from(&path).unwrap();
        assert!(back.warnings.is_empty(), "{:?}", back.warnings);
        assert_eq!(back.providers, c.providers);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let open = AiConfig::load_from(&path).unwrap();
        assert!(open.warnings[0].contains("chmod 600"), "{:?}", open.warnings);
        // saving again makes it private again
        open.save_to(&path).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        // XDG_CONFIG_HOME wins
        let xdg = |k: &str| match k {
            "XDG_CONFIG_HOME" => Some("/x/cfg".to_string()),
            "HOME" => Some("/x/home".to_string()),
            _ => None,
        };
        assert_eq!(path_from(&xdg).unwrap(), PathBuf::from("/x/cfg/omarchyvr/ai.toml"));
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn parsing_rules() {
        let t = "# comment\ndefault = 'claude' # inline\n[use]\nvoice = \"claude\"\ncooking = \"x\"\n[providers.claude]\nkind = \"anthropic\"\napi_key = \"a\\\"b\\\\c\"\nfuture = [1, 2]\n[weird]\nx = 1\n";
        let c = AiConfig::parse(t).unwrap();
        assert_eq!(c.default.as_deref(), Some("claude"));
        assert_eq!(c.providers[0].api_key.as_ref().unwrap().expose(), "a\"b\\c");
        assert_eq!(c.providers[0].extra.get("future").map(String::as_str), Some("[1, 2]"));
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        assert_eq!(c.use_extra.get("cooking").map(String::as_str), Some("\"x\""));
        assert_eq!(c.other_sections, vec![("weird".to_string(), vec![("x".to_string(), "1".to_string())])]);
        assert!(AiConfig::parse(&c.render()).unwrap().providers[0].extra.contains_key("future"), "unknown keys survive a save");
        assert!(AiConfig::parse("[providers.x]\nmodel = \"m\"\n").unwrap_err().contains("no kind"));
        assert!(AiConfig::parse("[providers.x]\nkind = \"gemini\"\n").unwrap_err().contains("kind"));
        assert!(AiConfig::parse("[providers.a b]\nkind = \"openai\"\n").is_err());
        assert!(AiConfig::parse("default = claude\n").is_err(), "strings are quoted");
        assert!(AiConfig::parse("default = \"x\" y\n").is_err());
        let d = AiConfig::parse("").unwrap();
        assert_eq!((d.voice_ai, d.strip_titles), (VoiceAi::Fallback, false), "the defaults");
        let v = AiConfig::parse("voice_ai = \"off\"\nstrip_titles = true # privacy\n").unwrap();
        assert_eq!((v.voice_ai, v.strip_titles), (VoiceAi::Off, true));
        assert!(AiConfig::parse("voice_ai = \"sometimes\"\n").is_err());
        assert_eq!(d.voice_confirm, VoiceConfirm::None, "no confirmation by default");
        let v = AiConfig::parse("voice_confirm = \"dangerous\"\n").unwrap();
        assert_eq!(v.voice_confirm, VoiceConfirm::Dangerous);
        assert!(v.render().contains("voice_confirm = \"dangerous\""));
        assert_eq!(AiConfig::parse(&v.render()).unwrap().voice_confirm, VoiceConfirm::Dangerous);
        assert!(AiConfig::parse("voice_confirm = \"maybe\"\n").is_err());
    }

    #[test]
    fn voice_listen_default_parse_render_and_mapping() {
        assert_eq!(AiConfig::default().voice_listen, VoiceListen::Long, "a bigger buffer by default");
        let v = AiConfig::parse("voice_listen = \"very-long\"\n").unwrap();
        assert_eq!(v.voice_listen, VoiceListen::VeryLong);
        assert!(v.render().contains("voice_listen = \"very-long\""));
        assert_eq!(AiConfig::parse(&v.render()).unwrap().voice_listen, VoiceListen::VeryLong);
        assert_eq!(AiConfig::parse(&AiConfig::default().render()).unwrap().voice_listen, VoiceListen::Long);
        assert!(AiConfig::parse("voice_listen = \"forever\"\n").is_err());
        let m: Vec<_> = VoiceListen::ALL.iter().map(|l| (l.endpoint_ms(), l.long_hangover_ms(), l.followup_s())).collect();
        assert_eq!(m, [(480, 1120, 6.0), (750, 1500, 8.0), (1100, 2000, 10.0)]);
        assert!(VoiceConfirm::Dangerous.asks(true) && !VoiceConfirm::Dangerous.asks(false) && !VoiceConfirm::None.asks(true) && VoiceConfirm::Always.asks(false));
        assert!(AiConfig::parse("strip_titles = yes\n").is_err());
        assert!(is_loopback("http://127.0.0.1:8080/v1") && is_loopback("http://localhost/v1") && is_loopback("http://[::1]:1/v1"));
        assert!(!is_loopback("https://api.openai.com/v1") && !is_loopback("http://192.168.1.5:1234/v1") && !is_loopback("http://127.0.0.1.evil.com/v1"));
    }

    #[test]
    fn lara_in_front_of_an_online_brain_and_what_the_ai_servers_have_to_do() {
        // Claude with a key + Lara: Lara runs (router only), no local model
        let key = "voice_lara = true\nvoice_local_models = false\n[use]\nvoice = \"claude\"\n[providers.claude]\nkind = \"anthropic\"\napi_key = \"sk-ant-TEST\"\n";
        let c = AiConfig::parse(key).unwrap();
        assert!(c.lara_with_online() && c.lara_on() && c.ai_servers_wanted() && !c.local_models_on());
        assert!(AiConfig::parse(&c.render()).unwrap().voice_lara == Some(true), "the key survives a save");
        // the Claude CLI with Lara: Lara goes first, Claude Code gets the rest
        let cli = AiConfig::parse("voice_lara = true\nvoice_local_models = false\n").unwrap();
        assert!(cli.lara_with_online() && cli.ai_servers_wanted(), "default brain = the Claude CLI, Lara in front");
        let cli_off = AiConfig::parse("voice_lara = false\nvoice_local_models = false\n").unwrap();
        assert!(!cli_off.lara_with_online() && !cli_off.ai_servers_wanted(), "without Lara everything goes to Claude Code");
        // Lara off with a key: nothing on this PC
        let off = AiConfig::parse(&key.replace("voice_lara = true", "voice_lara = false")).unwrap();
        assert!(!off.ai_servers_wanted() && !off.lara_on());
        // a local model in two stages: Lara is the local router as before
        let local = AiConfig::parse("voice_local = \"two-stage\"\nvoice_local_models = true\n[use]\nvoice = \"local\"\n[providers.local]\nkind = \"openai-compatible\"\nbase_url = \"http://127.0.0.1:8080/v1\"\nmodel = \"pimax-local\"\n").unwrap();
        assert!(local.lara_on() && local.ai_servers_wanted() && !local.lara_with_online());
    }
}
