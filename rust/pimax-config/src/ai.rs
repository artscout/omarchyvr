//! AI providers shared by the Pimax app (AI tab, Autofix) and voice control: `~/.config/pimax/ai.toml`.
//!
//! ```toml
//! default = "claude"              # the provider every feature uses unless [use] says otherwise
//! voice_ai = "fallback"           # off | fallback (when the offline matcher has no answer) | always
//! strip_titles = false            # privacy: window titles are not sent to a provider
//! voice_confirm = "none"          # none (default) | dangerous | always: when a voice command waits for "yes"
//! voice_ai_timeout_s = 4          # optional: how long voice waits for an answer (default 4 local, 8 remote)
//! voice_ai_gaze = true            # optional: send what you look at with a voice request (default true)
//! voice_ai_log = true             # optional: ~/.local/state/pimax/logs/ai.log (default true)
//!
//! [use]                           # optional per-feature choice
//! voice = "claude"
//! autofix = "claude"
//!
//! [providers.claude]
//! kind = "anthropic"              # anthropic | openai | openai-compatible
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
}

impl Kind {
    /// Every kind.
    pub const ALL: [Kind; 3] = [Kind::Anthropic, Kind::OpenAi, Kind::OpenAiCompatible];

    /// The value in the file.
    pub fn id(self) -> &'static str {
        match self {
            Kind::Anthropic => "anthropic",
            Kind::OpenAi => "openai",
            Kind::OpenAiCompatible => "openai-compatible",
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
        }
    }

    /// The base URL when the file sets none (none for compatible servers: they need one).
    pub fn default_base(self) -> Option<&'static str> {
        match self {
            Kind::Anthropic => Some(ANTHROPIC_BASE),
            Kind::OpenAi => Some(OPENAI_BASE),
            Kind::OpenAiCompatible => None,
        }
    }

    /// A key is required.
    pub fn needs_key(self) -> bool {
        self != Kind::OpenAiCompatible
    }

    /// The environment variable a key usually comes from.
    pub fn usual_env(self) -> Option<&'static str> {
        match self {
            Kind::Anthropic => Some("ANTHROPIC_API_KEY"),
            Kind::OpenAi => Some("OPENAI_API_KEY"),
            Kind::OpenAiCompatible => None,
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
    /// Keys this version does not know, raw values (kept on save).
    pub extra: BTreeMap<String, String>,
}

impl Provider {
    /// A provider with the usual settings of its kind.
    pub fn new(name: &str, kind: Kind) -> Provider {
        Provider {
            name: name.to_string(),
            kind,
            model: if kind == Kind::Anthropic { CLAUDE_MODELS[0].0.to_string() } else { String::new() },
            api_key: None,
            api_key_env: None,
            base_url: match kind {
                Kind::OpenAi => Some(OPENAI_BASE.into()),
                Kind::OpenAiCompatible => Some(OLLAMA_BASE.into()),
                Kind::Anthropic => None,
            },
            action_model: None,
            extra: BTreeMap::new(),
        }
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
}

impl Resolved {
    /// The URL a request goes to: Anthropic `<base>/v1/messages`, the others `<base>/chat/completions`.
    pub fn endpoint(&self) -> String {
        let b = self.base_url.trim_end_matches('/');
        match self.kind {
            Kind::Anthropic => format!("{b}/v1/messages"),
            _ => format!("{b}/chat/completions"),
        }
    }

    /// Requests stay on this machine (a loopback base URL).
    pub fn local(&self) -> bool {
        is_loopback(&self.base_url)
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
    /// Send what you look at with a voice request (`voice_ai_gaze`; None: yes).
    pub voice_ai_gaze: Option<bool>,
    /// Log AI requests to `~/.local/state/pimax/logs/ai.log` (`voice_ai_log`; None: yes).
    pub voice_ai_log: Option<bool>,
    /// With a local voice model: use the online provider when the local server is not running (`voice_online_backup`; None: yes).
    pub voice_online_backup: Option<bool>,
    /// Top-level keys this version does not know, raw values (kept on save).
    pub extra: BTreeMap<String, String>,
    /// `[use]` entries this version does not know (kept on save).
    pub use_extra: BTreeMap<String, String>,
    /// Sections this version does not know, with their raw `key = value` lines, in file order (kept on save).
    pub other_sections: Vec<(String, Vec<(String, String)>)>,
    /// What the last load noticed (an unsafe file mode, unknown lines).
    pub warnings: Vec<String>,
}

/// A provider name the file can hold.
pub fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 32 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Where the file is: `$XDG_CONFIG_HOME/pimax/ai.toml`, else `$HOME/.config/pimax/ai.toml`.
pub fn path_from(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let base = env("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).or_else(|| env("HOME").filter(|v| !v.is_empty()).map(|h| Path::new(&h).join(".config")))?;
    Some(base.join("pimax/ai.toml"))
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
                } else if let Some(name) = h.strip_prefix("providers.") {
                    let name = name.trim().trim_matches('"');
                    if !valid_name(name) {
                        return Err(format!("line {n}: provider names are letters, digits, - and _"));
                    }
                    if c.providers.iter().any(|p| p.name == name) {
                        return Err(format!("line {n}: provider {name} twice"));
                    }
                    c.providers.push(Provider { name: name.to_string(), kind: Kind::OpenAiCompatible, model: String::new(), api_key: None, api_key_env: None, base_url: None, action_model: None, extra: BTreeMap::new() });
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
                    "voice_confirm" => {
                        let m = unquote(v, n)?;
                        c.voice_confirm = VoiceConfirm::parse(&m).ok_or_else(|| format!("line {n}: voice_confirm {m:?} (none, dangerous or always)"))?;
                    }
                    "voice_ai_gaze" => c.voice_ai_gaze = Some(parse_bool(v, n, k)?),
                    "voice_ai_log" => c.voice_ai_log = Some(parse_bool(v, n, k)?),
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
                            p.kind = Kind::parse(&s).ok_or_else(|| format!("line {n}: kind {s:?} (anthropic, openai or openai-compatible)"))?;
                            kinds[*ix] = true;
                        }
                        "model" => p.model = unquote(v, n)?,
                        "api_key" => p.api_key = Some(Secret::new(&unquote(v, n)?)).filter(|s| !s.is_empty()),
                        "api_key_env" => p.api_key_env = Some(unquote(v, n)?).filter(|s| !s.is_empty()),
                        "base_url" => p.base_url = Some(unquote(v, n)?).filter(|s| !s.is_empty()),
                        "action_model" => p.action_model = Some(unquote(v, n)?).filter(|s| !s.trim().is_empty()),
                        _ => {
                            p.extra.insert(k.to_string(), v.to_string());
                        }
                    }
                }
                Section::Other(ix) => c.other_sections[*ix].1.push((k.to_string(), v.to_string())),
            }
        }
        if let Some(p) = c.providers.iter().zip(&kinds).find(|(_, k)| !**k).map(|(p, _)| p) {
            return Err(format!("provider {} has no kind (anthropic, openai or openai-compatible)", p.name));
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
        if let Some(t) = self.voice_ai_timeout_s {
            o += &format!("voice_ai_timeout_s = {t}\n");
        }
        if let Some(g) = self.voice_ai_gaze {
            o += &format!("voice_ai_gaze = {g}\n");
        }
        if let Some(l) = self.voice_ai_log {
            o += &format!("voice_ai_log = {l}\n");
        }
        if let Some(b) = self.voice_online_backup {
            o += &format!("voice_online_backup = {b}\n");
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
            for (k, v) in &p.extra {
                o += &format!("{k} = {v}\n");
            }
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

    /// A provider by name.
    pub fn provider(&self, name: &str) -> Option<&Provider> {
        self.providers.iter().find(|p| p.name == name)
    }

    /// The provider name a feature uses: its `[use]` entry, else `default`, else the only provider.
    pub fn provider_for(&self, f: Feature) -> Option<&Provider> {
        self.uses
            .get(&f)
            .and_then(|n| self.provider(n))
            .or_else(|| self.default.as_deref().and_then(|n| self.provider(n)))
            .or_else(|| if self.providers.len() == 1 { self.providers.first() } else { None })
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
        Ok(Resolved { provider: p.name.clone(), kind: p.kind, model, base_url, api_key, key_source, action_model: p.action_model.clone() })
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
        let d = std::env::temp_dir().join(format!("pimax-ai-{tag}-{}", std::process::id()));
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
        // no provider at all
        assert!(AiConfig::default().resolve_with(Feature::Voice, &none, None).is_err());
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
        assert_eq!(path, home.join(".config/pimax/ai.toml"));
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
        assert_eq!(path_from(&xdg).unwrap(), PathBuf::from("/x/cfg/pimax/ai.toml"));
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
        assert!(VoiceConfirm::Dangerous.asks(true) && !VoiceConfirm::Dangerous.asks(false) && !VoiceConfirm::None.asks(true) && VoiceConfirm::Always.asks(false));
        assert!(AiConfig::parse("strip_titles = yes\n").is_err());
        assert!(is_loopback("http://127.0.0.1:8080/v1") && is_loopback("http://localhost/v1") && is_loopback("http://[::1]:1/v1"));
        assert!(!is_loopback("https://api.openai.com/v1") && !is_loopback("http://192.168.1.5:1234/v1") && !is_loopback("http://127.0.0.1.evil.com/v1"));
    }
}
