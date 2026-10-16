//! The AI tab: the providers of `~/.config/pimax/ai.toml` (`pimax_config::ai`, shared with voice control), the default
//! and per-feature choice, keys kept for this session only, the connection test and the local-model probe.
//!
//! The file is read and written in the background (atomically, mode 0600, by `pimax_config::ai`); a file that does
//! not parse is never overwritten. The only network call is Test connection: the fixed [`llm::PING`] to that provider,
//! on click. The probe asks 127.0.0.1:11434 and :8080 only, when the tab opens or on click.
use crate::app::AppCore;
use crate::llm::{self, Backend, Request};
use pimax_config::ai::{self, AiConfig, Feature, Kind, Provider, Resolved, Secret, VoiceAi};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// The job kind of the voice command audit.
pub const VOICE_AUDIT_KIND: &str = "voice-audit";

/// The AI tab's state.
#[derive(Debug, Default)]
pub struct AiView {
    /// The providers (empty until read).
    pub config: AiConfig,
    /// The file was read.
    pub loaded: bool,
    /// Why the file could not be read (then it is not overwritten).
    pub error: Option<String>,
    /// Keys kept for this session only (never written).
    pub session_keys: BTreeMap<String, Secret>,
    /// The last connection test per provider: the answer, or why it failed.
    pub tests: BTreeMap<String, Result<String, String>>,
    /// Tests running.
    pub testing: BTreeSet<String>,
    /// Local servers found by the probe.
    pub found: Vec<Backend>,
    /// The probe ran.
    pub probed: bool,
    /// The probe runs.
    pub probing: bool,
    /// The last save's result for people.
    pub saved: String,
    /// The newest lines of `~/.local/state/pimax/logs/ai.log` (the AI actions of voice control), newest first.
    pub log: Vec<String>,
    /// When the log was read last.
    log_at: Option<Instant>,
    /// The latest config to write (the writer task takes it).
    slot: Arc<Mutex<Option<AiConfig>>>,
}

/// The demo providers (demo mode: nothing is read or written, nothing is sent).
pub fn demo_config() -> AiConfig {
    let mut claude = Provider::new("claude", Kind::Anthropic);
    claude.api_key = Some(Secret::new("sk-ant-demo-0000000000000000"));
    let mut local = Provider::new("local", Kind::OpenAiCompatible);
    local.model = "qwen3:8b".into();
    let mut openrouter = Provider::new("openrouter", Kind::OpenAiCompatible);
    openrouter.base_url = Some(pimax_config::ai::OPENROUTER_BASE.into());
    openrouter.model = pimax_config::ai::OPENROUTER_VOICE_MODEL.into();
    openrouter.api_key = Some(Secret::new("sk-or-v1-demo-0000000000000000"));
    openrouter.action_model = Some(pimax_config::ai::OPENROUTER_ACTION_MODELS[0].into());
    AiConfig {
        default: Some("claude".into()),
        uses: BTreeMap::from([(Feature::Autofix, "claude".to_string()), (Feature::Voice, "openrouter".to_string())]),
        providers: vec![claude, local, openrouter],
        ..Default::default()
    }
}

/// What the editor hands back for a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyChoice {
    /// No key (a local server, or the environment variable).
    None,
    /// Store it in ai.toml.
    Save(Secret),
    /// Keep it in memory for this session only.
    Session(Secret),
}

/// "AI runs on" (the AI page's first switch, also on the Work desk's watch).
pub use pimax_config::ai::RunsOn;

impl AppCore {
    /// ai.toml.
    pub fn ai_path(&self) -> PathBuf {
        self.paths.config.join("ai.toml")
    }

    /// Read the file once and look for local servers once (both in the background).
    pub fn want_ai(&mut self) {
        if !self.ai.loaded && !self.task_busy("ai-load") {
            if self.demo {
                self.ai.config = demo_config();
                self.ai.loaded = true;
            } else {
                let path = self.ai_path();
                self.task(Some("ai-load"), move || {
                    let r = AiConfig::load_from(&path);
                    if let Ok(c) = &r {
                        for w in &c.warnings {
                            log::warn!("ai.toml: {w}");
                        }
                    }
                    Box::new(move |c: &mut AppCore| {
                        match r {
                            Ok(cfg) => {
                                c.ai.config = cfg;
                                c.ai.error = None;
                            }
                            Err(e) => c.ai.error = Some(e),
                        }
                        c.ai.loaded = true;
                    })
                });
            }
        }
        if !self.ai.probed && !self.ai.probing {
            self.ai_probe();
        }
    }

    /// Look for local servers (GETs to 127.0.0.1:11434 and :8080 only, a short timeout).
    pub fn ai_probe(&mut self) {
        self.ai.probing = true;
        let t = self.autofix.transport.clone();
        self.task(Some("ai-probe"), move || {
            let found = t.probe();
            Box::new(move |c: &mut AppCore| {
                c.ai.found = found;
                c.ai.probed = true;
                c.ai.probing = false;
            })
        });
    }

    /// An environment variable of the app (the UI shows where keys come from).
    pub fn env_var(&self, k: &str) -> Option<String> {
        (self.env)(k)
    }

    /// What a feature uses now (a session key counts).
    pub fn ai_resolve(&self, f: Feature) -> Result<Resolved, String> {
        let name = self.ai.config.provider_for(f).map(|p| p.name.clone());
        let session = name.as_deref().and_then(|n| self.ai.session_keys.get(n));
        self.ai.config.resolve_with(f, &|k| (self.env)(k), session)
    }

    /// One provider as a request would use it.
    pub fn ai_resolve_provider(&self, name: &str) -> Result<Resolved, String> {
        self.ai.config.resolve_provider(name, &|k| (self.env)(k), self.ai.session_keys.get(name))
    }

    /// Every key the app knows (the file, the environment, this session): the redactor removes them all.
    pub fn ai_known_keys(&self) -> Vec<Secret> {
        let mut v: Vec<Secret> = self.ai.session_keys.values().cloned().collect();
        for p in &self.ai.config.providers {
            v.extend(p.api_key.clone());
            if let Some(k) = p.api_key_env.as_deref().or(p.kind.usual_env()).and_then(|e| (self.env)(e)) {
                v.push(Secret::new(&k));
            }
        }
        v
    }

    /// Add or change a provider (`old`: its name before an edit) and save.
    pub fn ai_put(&mut self, old: Option<&str>, mut p: Provider, key: KeyChoice) -> Result<(), String> {
        p.name = p.name.trim().to_string();
        if !ai::valid_name(&p.name) {
            return Err("Names are letters, digits, - and _ (up to 32).".into());
        }
        if old != Some(p.name.as_str()) && self.ai.config.provider(&p.name).is_some() {
            return Err(format!("There is already a provider named {}.", p.name));
        }
        p.model = p.model.trim().to_string();
        if p.model.is_empty() && p.kind != Kind::Anthropic {
            return Err("Enter the model name.".into());
        }
        p.base_url = p.base_url.map(|b| b.trim().to_string()).filter(|b| !b.is_empty());
        if let Some(b) = &p.base_url {
            if !(b.starts_with("http://") || b.starts_with("https://")) || b.contains(['"', '\\', ' ']) {
                return Err("The base URL starts with http:// or https://.".into());
            }
        } else if p.kind == Kind::OpenAiCompatible {
            return Err("A local or other server needs its base URL (e.g. http://127.0.0.1:11434/v1).".into());
        }
        p.api_key_env = p.api_key_env.map(|e| e.trim().to_string()).filter(|e| !e.is_empty());
        if p.api_key_env.as_deref().is_some_and(|e| !e.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')) {
            return Err("An environment variable name is letters, digits and _.".into());
        }
        self.ai.session_keys.remove(&p.name);
        if let Some(o) = old {
            self.ai.session_keys.remove(o);
        }
        match key {
            KeyChoice::None => p.api_key = None,
            KeyChoice::Save(k) => p.api_key = Some(k),
            KeyChoice::Session(k) => {
                p.api_key = None;
                self.ai.session_keys.insert(p.name.clone(), k);
            }
        }
        let c = &mut self.ai.config;
        match old.and_then(|o| c.providers.iter().position(|x| x.name == o)) {
            Some(i) => {
                let was = c.providers[i].name.clone();
                if was != p.name {
                    if c.default.as_deref() == Some(was.as_str()) {
                        c.default = Some(p.name.clone());
                    }
                    for v in c.uses.values_mut().filter(|v| **v == was) {
                        *v = p.name.clone();
                    }
                    self.ai.tests.remove(&was);
                }
                c.providers[i] = p;
            }
            None => {
                if c.providers.is_empty() {
                    c.default = Some(p.name.clone());
                }
                c.providers.push(p);
            }
        }
        self.ai_save();
        self.autofix.preview = None;
        Ok(())
    }

    /// Remove a provider (and the choices that named it) and save.
    pub fn ai_remove(&mut self, name: &str) {
        let c = &mut self.ai.config;
        c.providers.retain(|p| p.name != name);
        if c.default.as_deref() == Some(name) {
            c.default = c.providers.first().map(|p| p.name.clone());
        }
        c.uses.retain(|_, v| v != name);
        self.ai.session_keys.remove(name);
        self.ai.tests.remove(name);
        self.ai_save();
    }

    /// The default provider.
    pub fn ai_set_default(&mut self, name: &str) {
        if self.ai.config.provider(name).is_some() {
            self.ai.config.default = Some(name.to_string());
            self.ai_save();
        }
    }

    /// A feature's provider (None: the default).
    pub fn ai_set_use(&mut self, f: Feature, name: Option<&str>) {
        match name.filter(|n| self.ai.config.provider(n).is_some()) {
            Some(n) => {
                self.ai.config.uses.insert(f, n.to_string());
            }
            None => {
                self.ai.config.uses.remove(&f);
            }
        }
        self.ai_save();
        if f == Feature::Autofix {
            self.autofix.sized = None;
        }
    }

    /// The first provider on this PC (loopback base URL), by name.
    pub fn ai_local_provider(&self) -> Option<String> {
        self.ai.config.providers.iter().find(|p| p.base().is_some_and(ai::is_loopback)).map(|p| p.name.clone())
    }

    /// The first online provider (OpenRouter first), by name.
    pub fn ai_online_provider(&self) -> Option<String> {
        let on = || self.ai.config.providers.iter().filter(|p| p.base().is_some_and(|b| !ai::is_loopback(b)));
        on().find(|p| p.base().is_some_and(|b| b.contains("openrouter"))).or_else(|| on().next()).map(|p| p.name.clone())
    }

    /// Voice runs online (its `[use]` provider is not on this PC).
    pub fn ai_voice_online(&self) -> bool {
        self.ai.config.provider_for(Feature::Voice).is_some_and(|p| p.base().is_some_and(|b| !ai::is_loopback(b)))
    }

    /// "AI runs on": This PC (local) or Online. Writes `[use] voice` only; keys stay as they are.
    pub fn ai_set_runs_on(&mut self, online: bool) {
        let n = if online { self.ai_online_provider() } else { self.ai_local_provider() };
        self.ai_set_use(Feature::Voice, n.as_deref());
    }

    /// "AI runs on" with three choices: This PC (simple), This PC (two stages: Lara + GPU model), Online.
    pub fn ai_runs_on(&self) -> RunsOn {
        self.ai.config.runs_on()
    }

    /// Switch "AI runs on": `[use] voice` and `voice_local` only (keys and other providers stay as they are; a local
    /// choice adds the local provider when it is missing). The running local servers follow (`pimax-ai up`), and the
    /// running voice control picks the new file up by itself.
    pub fn ai_set_runs_on3(&mut self, r: RunsOn) {
        if let Err(e) = self.ai.config.set_runs_on(r) {
            self.message = e;
            return;
        }
        self.ai_save();
        if r != RunsOn::Online {
            self.localai_follow();
        }
    }

    /// "Local models: On / Off" (Work only). Off with `online`: Jev online instead; Off without: the offline
    /// commands only. The local servers follow when they run.
    pub fn ai_set_local_models(&mut self, on: bool, online: bool) {
        if let Err(e) = self.ai.config.set_local_models(on, online) {
            self.message = e;
            return;
        }
        if on && self.ai_local_provider().is_none() {
            if let Err(e) = self.localai_use() {
                self.message = e;
            }
        }
        self.ai_save();
        self.localai_follow();
    }

    /// Low / Medium / High: the preset's router and action model.
    pub fn ai_set_preset(&mut self, p: ai::Preset) {
        self.ai.config.set_preset(p);
        self.ai_save();
        self.localai_follow();
    }

    /// The two-stage router ("Lara") or action model: an id of `ROUTER_MODELS` / `ACTION_MODELS`.
    pub fn ai_set_two_stage_model(&mut self, router: bool, id: &str) {
        let t = self.ai.config.local_two_stage.get_or_insert_with(ai::LocalTwoStage::default);
        if router {
            t.router = id.into();
        } else {
            t.action = id.into();
        }
        // chosen by hand: a preset only when the pair is one
        t.preset = t.matching();
        self.ai_save();
        self.localai_follow();
    }

    /// "VRAM for AI": None = Auto, Some(MiB) = up to that much.
    pub fn ai_set_vram(&mut self, mib: Option<u32>) {
        self.ai.config.local_two_stage.get_or_insert_with(ai::LocalTwoStage::default).vram_mib = mib;
        self.ai_save();
        self.localai_follow();
    }

    /// "Speech recognition": Auto / Small (processor) / Large (graphics card), saved as `asr` (docs/AI.md section 17).
    /// The speech server follows at once (`pimax-ai up`); voice control picks the engine when it starts.
    pub fn ai_set_asr(&mut self, a: ai::Asr) {
        self.ai.config.asr = a;
        self.ai_save();
        self.localai_follow();
    }

    /// Agent mode (`voice_agent`, default on): a goal of several steps runs step by step (docs/VOICE.md). Kept as a
    /// top-level key of ai.toml (`AiConfig.extra`), read by pimax-voice act.
    pub fn ai_voice_agent(&self) -> bool {
        self.ai.config.extra.get("voice_agent").is_none_or(|v| v.trim() != "false")
    }

    pub fn ai_set_voice_agent(&mut self, on: bool) {
        self.ai.config.extra.insert("voice_agent".into(), on.to_string());
        self.ai_save();
    }

    /// The optional big agent-mode model (Qwen3.6-27B, `[local_two_stage] agent`; docs/AI.md section 18): opt-in,
    /// default off.
    pub fn ai_agent_big(&self) -> bool {
        self.ai.config.local_two_stage.as_ref().and_then(|t| t.agent.as_deref()).is_some()
    }

    pub fn ai_set_agent_big(&mut self, on: bool) {
        match self.ai.config.set_agent(if on { Some("qwen3.6-27b") } else { None }) {
            Ok(()) => {
                self.ai_save();
                self.localai_follow();
            }
            Err(e) => self.ai.saved = format!("Agent model: {e}"),
        }
    }

    /// "Use online if local is not running" (`voice_online_backup`).
    pub fn ai_set_online_backup(&mut self, on: bool) {
        self.ai.config.voice_online_backup = Some(on);
        self.ai_save();
    }

    /// An error of a connection test in plain words.
    pub fn ai_plain_error(e: &str) -> String {
        let l = e.to_ascii_lowercase();
        if l.contains("401") || l.contains("403") || l.contains("expired") || l.contains("api key") {
            "The online key was refused (expired or wrong). Open Developer mode > AI > Model and key and paste a new key.".into()
        } else if l.contains("timed out") {
            "No answer in time. The model is slow or busy.".into()
        } else if l.contains("refused") || l.contains("connect") || l.contains("curl") {
            "Cannot reach it. The local model is not running (start it below) or there is no internet.".into()
        } else {
            e.to_string()
        }
    }

    /// Write the file in the background (latest wins). Never over a file that could not be read; nothing in demo mode.
    pub fn ai_save(&mut self) {
        if self.demo {
            self.ai.saved = "demo mode: ai.toml is not written".into();
            return;
        }
        if let Some(e) = &self.ai.error {
            self.ai.saved = format!("Not saved: the file could not be read ({e}); fix or remove it first.");
            return;
        }
        if let Ok(mut s) = self.ai.slot.lock() {
            *s = Some(self.ai.config.clone());
        }
        if self.task_busy("ai-save") {
            return;
        }
        let (slot, path) = (self.ai.slot.clone(), self.ai_path());
        self.task(Some("ai-save"), move || {
            let mut r = Ok(());
            loop {
                let next = slot.lock().ok().and_then(|mut s| s.take());
                let Some(c) = next else { break };
                r = c.save_to(&path);
            }
            if r.is_ok() {
                log::info!("ai.toml saved");
            }
            Box::new(move |c: &mut AppCore| {
                // the local AI follows a changed setting once the file is written (pimax-ai up reads it)
                if r.is_ok() && std::mem::take(&mut c.localai.follow_after_save) {
                    c.localai_follow_now();
                }
                c.ai.saved = match r {
                    Ok(()) => format!("Saved to {} (mode 600)", c.ai_path().display()),
                    Err(e) => format!("Not saved: {e}"),
                };
            })
        });
    }

    /// Test a provider: the fixed PING, on this click only (the one network call of the tab).
    pub fn ai_test(&mut self, name: &str) {
        if self.ai.testing.contains(name) {
            return;
        }
        let r = match self.ai_resolve_provider(name) {
            Ok(r) => r,
            Err(e) => {
                self.ai.tests.insert(name.to_string(), Err(e));
                return;
            }
        };
        let req = Request::ping(Backend::from_resolved(&r), r.api_key.clone(), self.paths.state_dir.join("ai"));
        self.ai.testing.insert(name.to_string());
        self.ai.tests.remove(name);
        let t = self.autofix.transport.clone();
        let n = name.to_string();
        let t0 = Instant::now();
        log::info!("AI connection test: {n} ({}, {})", r.kind.id(), r.model);
        self.task(None, move || {
            let res = t.send(&req).map(|x| {
                let said: String = x.text.trim().chars().take(40).collect();
                let cost = x.cost.as_deref().map(|c| format!(", {c} per request")).unwrap_or_default();
                format!("OK: {} answered \"{said}\" in {:.1} s{cost}", x.model, t0.elapsed().as_secs_f32())
            });
            log::info!("AI connection test {n}: {}", if res.is_ok() { "ok" } else { "failed" });
            Box::new(move |c: &mut AppCore| {
                c.ai.testing.remove(&n);
                c.ai.tests.insert(n, res);
            })
        });
    }

    /// How voice control uses a model.
    pub fn ai_set_voice_ai(&mut self, v: VoiceAi) {
        self.ai.config.voice_ai = v;
        self.ai_save();
    }

    /// The optional voice settings (None: the agent's default).
    pub fn ai_set_advanced(&mut self, timeout_s: Option<f64>, gaze: Option<bool>, log: Option<bool>) {
        let c = &mut self.ai.config;
        c.voice_ai_timeout_s = timeout_s.filter(|t| t.is_finite() && *t > 0.0 && *t <= 600.0);
        c.voice_ai_gaze = gaze;
        c.voice_ai_log = log;
        self.ai_save();
    }

    /// When a voice command waits for a spoken "yes".
    pub fn ai_set_voice_confirm(&mut self, v: pimax_config::ai::VoiceConfirm) {
        self.ai.config.voice_confirm = v;
        self.ai_save();
    }

    /// Where the voice command audit writes its report.
    pub fn voice_audit_report(&self) -> PathBuf {
        self.paths.state_dir.join("voice-audit.html")
    }

    /// Check every voice command (`pimax-voice audit`, dry run: nothing is executed, typed or clicked) against the
    /// voice provider, at nice 19 with a pause between model requests. Refused while a VR session runs: the audit
    /// keeps the local model busy for a few minutes, which the live voice commands need.
    pub fn voice_audit(&mut self) {
        if self.live.service.is_some() {
            self.message = "The voice check runs between VR sessions (it keeps the voice model busy for a few minutes).".into();
            return;
        }
        let bin = self.paths.bin("pimax-voice");
        let bin = if bin.is_file() { bin.display().to_string() } else { "pimax-voice".to_string() };
        let out = self.voice_audit_report().display().to_string();
        let c = crate::cmd::Cmd::new("nice", &["-n", "19", &bin, "audit", "--pause-ms", "1000", "--out", &out]);
        self.spawn(crate::job::JobSpec { kind: crate::job::JobKind::Other(VOICE_AUDIT_KIND.into()), label: "Check all voice commands (dry run)".into(), steps: vec![crate::job::Step::Run(c)] });
    }

    /// Privacy: do not send window titles.
    pub fn ai_set_strip_titles(&mut self, on: bool) {
        self.ai.config.strip_titles = on;
        self.ai_save();
    }

    /// The AI actions log (`<state>/logs/ai.log`), read in the background at most every 5 s while shown.
    pub fn want_ai_log(&mut self) {
        if self.ai.log_at.is_some_and(|t| t.elapsed().as_secs() < 5) || self.task_busy("ai-log") {
            return;
        }
        self.ai.log_at = Some(Instant::now());
        if self.demo {
            self.ai.log = vec![
                "12:04:10 voice \"open the browser on the left screen\" -> claude: @win focus firefox (ok)".into(),
                "12:01:52 voice \"what's on my calendar\" -> claude: not a command (declined)".into(),
            ];
            return;
        }
        let path = self.paths.state_dir.join("logs/ai.log");
        self.task(Some("ai-log"), move || {
            let mut v = crate::applog::tail_file(&path, 30);
            v.reverse();
            Box::new(move |c: &mut AppCore| c.ai.log = v)
        });
    }

    /// Add a detected local server as a provider (named after the server).
    pub fn ai_add_found(&mut self, i: usize) -> Result<(), String> {
        let b = self.ai.found.get(i).cloned().ok_or("gone")?;
        let mut name = b.provider.clone();
        let mut n = 2;
        while self.ai.config.provider(&name).is_some() {
            name = format!("{}-{n}", b.provider);
            n += 1;
        }
        let mut p = Provider::new(&name, Kind::OpenAiCompatible);
        p.base_url = Some(b.base_url.clone());
        p.model = b.model.clone();
        self.ai_put(None, p, KeyChoice::None)
    }
}

/// Where a provider's key comes from, for people.
pub fn key_status(c: &AiView, p: &Provider, env: &dyn Fn(&str) -> Option<String>) -> (String, bool) {
    if p.api_key.is_some() {
        return ("key saved in ai.toml".into(), true);
    }
    if c.session_keys.contains_key(&p.name) {
        return ("key kept for this session only (not saved)".into(), true);
    }
    if let Some(e) = p.api_key_env.as_deref().or(p.kind.usual_env()) {
        if env(e).is_some_and(|v| !v.trim().is_empty()) {
            return (format!("key from ${e}"), true);
        }
        if p.api_key_env.is_some() {
            return (format!("key from ${e}, which is not set"), false);
        }
    }
    if p.kind.needs_key() {
        ("no key yet".into(), false)
    } else {
        ("no key needed".into(), true)
    }
}

/// Re-export for the UI.
pub use llm::PING as TEST_PROMPT;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::autofix::{tests::Counting, AutofixView};
    use pimax_config::Store;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn app(tag: &str) -> (AppCore, Arc<Counting>) {
        let d = std::env::temp_dir().join(format!("pimax-ai-tab-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let mut a = AppCore::demo(false, Store::new(d.join("cfg"), d.join("cache")));
        // a real (temporary) config directory: reads and writes go through the background tasks (inline here)
        a.demo = false;
        a.paths.config = d.join("cfg");
        a.paths.state_dir = d.join("state");
        let t = Arc::new(Counting { probes: AtomicUsize::new(0), sends: AtomicUsize::new(0), reply: "OK".into() });
        a.autofix = AutofixView::new(t.clone());
        (a, t)
    }

    #[test]
    fn runs_on_switches_the_saved_file_and_keeps_the_keys() {
        let (mut a, _) = app("runs-on");
        a.want_ai();
        let mut or = pimax_config::ai::Provider::new("openrouter", Kind::OpenAiCompatible);
        or.base_url = Some(ai::OPENROUTER_BASE.into());
        or.api_key = Some(Secret::new("sk-or-v1-TEST-NOT-A-KEY"));
        a.ai.config.providers.push(or);
        a.ai_set_runs_on3(RunsOn::TwoStage);
        assert_eq!(a.ai_runs_on(), RunsOn::TwoStage);
        a.ai_set_two_stage_model(false, "qwen3.5-9b");
        a.ai_set_vram(Some(6144));
        let text = std::fs::read_to_string(a.ai_path()).unwrap();
        assert!(text.contains("voice_local = \"two-stage\"") && text.contains("action = \"qwen3.5-9b\"") && text.contains("vram_mib = 6144"), "{text}");
        assert!(text.contains("sk-or-v1-TEST-NOT-A-KEY"), "the key stays in the file");
        a.ai_set_runs_on3(RunsOn::Online);
        assert_eq!(a.ai_runs_on(), RunsOn::Online);
        a.ai_set_runs_on3(RunsOn::Simple);
        assert_eq!(a.ai_runs_on(), RunsOn::Simple);
        assert!(std::fs::read_to_string(a.ai_path()).unwrap().contains("[local_two_stage]"), "the model choices are kept for next time");
        // the Local AI page: On / Off and the presets
        a.ai_set_local_models(true, false);
        a.ai_set_preset(ai::Preset::High);
        let text = std::fs::read_to_string(a.ai_path()).unwrap();
        assert!(text.contains("voice_local_models = true") && text.contains("preset = \"high\"") && text.contains("action = \"qwen3.5-9b\""), "{text}");
        // speech recognition: saved as `asr`, the keys stay
        a.ai_set_asr(ai::Asr::Gpu);
        let text = std::fs::read_to_string(a.ai_path()).unwrap();
        assert!(text.contains("asr = \"gpu\"") && text.contains("sk-or-v1-TEST-NOT-A-KEY"), "{text}");
        a.ai_set_asr(ai::Asr::Auto);
        assert!(!std::fs::read_to_string(a.ai_path()).unwrap().contains("asr ="), "Auto is the absent key");
        a.ai_set_local_models(false, true);
        assert_eq!(a.ai_runs_on(), RunsOn::Online);
        assert!(std::fs::read_to_string(a.ai_path()).unwrap().contains("sk-or-v1-TEST-NOT-A-KEY"));
    }

    #[test]
    fn the_voice_check_waits_for_the_end_of_a_vr_session() {
        let (mut a, _) = app("audit");
        a.live.service = Some(crate::facts::ServiceInfo { pid: 1, age: None, build: None, hz: Some(72), eye: false, sixdof: true, ctrl_track: false, broker: true, ipd: None, anchor: false });
        a.voice_audit();
        assert!(a.job.is_none(), "no audit job during a VR session");
        assert!(a.message.contains("between VR sessions"));
        assert!(a.voice_audit_report().ends_with("voice-audit.html"));
    }

    #[test]
    fn providers_are_saved_privately_and_tested_only_on_click() {
        use std::os::unix::fs::PermissionsExt;
        let (mut a, t) = app("save");
        a.want_ai();
        assert!(a.ai.loaded && a.ai.config.providers.is_empty());
        assert_eq!(t.probes.load(Ordering::SeqCst), 1, "the probe ran once, when the tab opened");
        a.want_ai();
        assert_eq!(t.probes.load(Ordering::SeqCst), 1);
        // a Claude provider with a saved key: the first one becomes the default
        a.ai_put(None, Provider::new("claude", Kind::Anthropic), KeyChoice::Save(Secret::new("sk-ant-api03-SAVED-TEST-NOT-A-KEY"))).unwrap();
        let path = a.ai_path();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("api_key = \"sk-ant-api03-SAVED-TEST-NOT-A-KEY\"") && text.contains("default = \"claude\""));
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        // an OpenAI provider whose key is kept for this session only: not in the file
        let mut o = Provider::new("openai", Kind::OpenAi);
        o.model = "gpt-test".into();
        a.ai_put(None, o, KeyChoice::Session(Secret::new("sk-proj-SESSION-TEST-NOT-A-KEY"))).unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().contains("SESSION"));
        a.ai_set_use(Feature::Autofix, Some("openai"));
        let r = a.ai_resolve(Feature::Autofix).unwrap();
        assert_eq!((r.provider.as_str(), r.key_source.clone()), ("openai", pimax_config::ai::KeySource::Session));
        assert_eq!(a.ai_resolve(Feature::Voice).unwrap().provider, "claude");
        assert_eq!(t.sends.load(Ordering::SeqCst), 0, "nothing was sent while editing");
        // Test connection: one request, the fixed prompt
        a.ai_test("openai");
        assert_eq!(t.sends.load(Ordering::SeqCst), 1);
        assert!(a.ai.tests["openai"].as_ref().unwrap().starts_with("OK: mock answered \"OK\""));
        // a rename follows the choices; a removal drops them
        let mut c = a.ai.config.provider("openai").unwrap().clone();
        c.name = "gpt".into();
        a.ai_put(Some("openai"), c, KeyChoice::None).unwrap();
        assert_eq!(a.ai.config.uses.get(&Feature::Autofix).map(String::as_str), Some("gpt"));
        a.ai_remove("gpt");
        assert!(a.ai.config.uses.is_empty());
        // names, models and URLs are checked
        assert!(a.ai_put(None, Provider::new("bad name", Kind::Anthropic), KeyChoice::None).is_err());
        assert!(a.ai_put(None, Provider::new("claude", Kind::Anthropic), KeyChoice::None).is_err(), "taken");
        let mut l = Provider::new("local", Kind::OpenAiCompatible);
        assert!(a.ai_put(None, l.clone(), KeyChoice::None).unwrap_err().contains("model"));
        l.model = "m".into();
        l.base_url = Some("ftp://x".into());
        assert!(a.ai_put(None, l, KeyChoice::None).is_err());
        // a detected server becomes a provider
        a.ai_add_found(0).unwrap();
        assert_eq!(a.ai.config.provider("llama-server").map(|p| p.model.as_str()), Some("test.gguf"));
        // a file that does not parse is never overwritten
        std::fs::write(&path, "[providers.x]\nkind = \"nope\"\n").unwrap();
        let (mut b, _) = app("save-bad");
        b.paths.config = path.parent().unwrap().to_path_buf();
        b.want_ai();
        assert!(b.ai.error.is_some());
        b.ai_set_default("x");
        b.ai_remove("x");
        assert!(b.ai.saved.starts_with("Not saved"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[providers.x]\nkind = \"nope\"\n");
        let _ = std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("pimax-ai-tab-save-bad-{}", std::process::id())));
    }

    #[test]
    fn key_status_says_where_a_key_comes_from() {
        let v = AiView::default();
        let env = |k: &str| (k == "ANTHROPIC_API_KEY").then(|| "k".to_string());
        let p = Provider::new("c", Kind::Anthropic);
        assert_eq!(key_status(&v, &p, &env), ("key from $ANTHROPIC_API_KEY".to_string(), true));
        assert_eq!(key_status(&v, &p, &|_| None), ("no key yet".to_string(), false));
        let l = Provider::new("l", Kind::OpenAiCompatible);
        assert_eq!(key_status(&v, &l, &|_| None).0, "no key needed");
    }
}
