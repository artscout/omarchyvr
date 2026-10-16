//! AI: the providers voice control and Autofix use (`~/.config/pimax/ai.toml`, shared with pimax-voice): add, edit,
//! remove, a masked key with show/hide (saved, or kept for this session only), Test connection (the one network call
//! here, on click: a fixed one-line prompt, no data from this PC), the default and per-feature provider, and the
//! local models found on 127.0.0.1.
use crate::app::PimaxApp;
use crate::theme::{self, icon};
use crate::widgets::{self, card, caption, chip, inset, note, pal, spinner, title};
use eframe::egui::{self, RichText, Ui};
use pimax_config::ai::{Feature, Kind, Provider, Secret, VoiceAi, VoiceConfirm, CLAUDE_MODELS};
use pimax_gui_core::ai::{key_status, KeyChoice, RunsOn, TEST_PROMPT};

/// The provider being added or edited (UI state; the key text is wiped when the editor closes).
#[derive(Debug, Clone, Default)]
pub struct AiDraft {
    /// The name before the edit (None: a new provider).
    pub old: Option<String>,
    /// Name.
    pub name: String,
    /// Kind.
    pub kind: usize,
    /// Model (with an action model: the intent model, which sorts voice commands).
    pub model: String,
    /// Voice control's action model (acts with tools); empty: the model above does both.
    pub action_model: String,
    /// Base URL.
    pub base_url: String,
    /// The key as typed.
    pub key: String,
    /// Show the key.
    pub show: bool,
    /// Save it in ai.toml (else: this session only).
    pub save_key: bool,
    /// Or read it from this environment variable.
    pub key_env: String,
    /// What was wrong with the last Save.
    pub error: String,
}

impl AiDraft {
    fn new(kind: Kind, name: &str) -> AiDraft {
        let p = Provider::new(name, kind);
        AiDraft {
            name: name.into(),
            kind: Kind::ALL.iter().position(|k| *k == kind).unwrap_or(0),
            model: p.model,
            base_url: p.base_url.unwrap_or_default(),
            save_key: true,
            ..Default::default()
        }
    }

    fn edit(p: &Provider, session: Option<&Secret>) -> AiDraft {
        let key = p.api_key.as_ref().or(session);
        AiDraft {
            old: Some(p.name.clone()),
            name: p.name.clone(),
            kind: Kind::ALL.iter().position(|k| *k == p.kind).unwrap_or(0),
            model: p.model.clone(),
            action_model: p.action_model.clone().unwrap_or_default(),
            base_url: p.base_url.clone().unwrap_or_default(),
            key: key.map(|k| k.expose().to_string()).unwrap_or_default(),
            show: false,
            save_key: session.is_none() || p.api_key.is_some(),
            key_env: p.api_key_env.clone().unwrap_or_default(),
            error: String::new(),
        }
    }

    /// The editor as a screenshot shows it (the key hidden).
    pub fn for_shot(p: &Provider) -> AiDraft {
        AiDraft::edit(p, None)
    }

    fn wipe(&mut self) {
        let n = self.key.len();
        // same length: the bytes are overwritten in place
        self.key.replace_range(.., &"\0".repeat(n));
        self.key.clear();
    }
}

fn unique(app: &PimaxApp, base: &str) -> String {
    let mut name = base.to_string();
    let mut n = 2;
    while app.core.ai.config.provider(&name).is_some() {
        name = format!("{base}-{n}");
        n += 1;
    }
    name
}

/// Draw the page.
pub fn show(ui: &mut Ui, app: &mut PimaxApp) {
    app.core.want_ai();
    let p = theme::pal(ui);
    card(ui, |ui| {
        ui.horizontal(|ui| {
            title(ui, "AI");
            widgets::help(ui, "The language models voice control and Autofix use: Claude, OpenAI, any OpenAI-compatible service, or a model running on this PC (ollama, llama-server, LM Studio).");
            if app.core.demo {
                chip(ui, "demo: nothing is read, written or sent", pal(ui).info);
            }
        });
        ui.horizontal(|ui| {
            caption(ui, "What is sent where");
            widgets::help(ui, &format!("This page sends nothing by itself. Test connection sends one fixed line (\"{TEST_PROMPT}\") to that provider, nothing from this PC. Autofix sends its redacted bundle (Setup > Autofix shows it) only when you press Send. Voice control sends the words you say to the provider picked for it. A local model keeps everything on this PC.\n\nKeys are saved in ~/.config/pimax/ai.toml, readable only by you (mode 600), or kept for this session only when you untick \"Save the key\" (then voice control, a separate program, cannot use them)."));
        });
        if let Some(e) = &app.core.ai.error {
            ui.label(RichText::new(format!("{e}: fix or remove the file; the app does not overwrite it.")).color(p.fail));
        }
        for w in &app.core.ai.config.warnings {
            ui.label(RichText::new(w).size(12.0).color(p.warn));
        }
        if !app.core.ai.saved.is_empty() {
            ui.label(RichText::new(&app.core.ai.saved).size(11.5).color(p.faint));
        }
    });
    if !app.core.ai.loaded {
        card(ui, |ui| {
            ui.horizontal(|ui| {
                spinner(ui);
                ui.label("Reading ~/.config/pimax/ai.toml...");
            });
        });
        return;
    }
    runs_on(ui, app);
    if !app.core.state.dev_mode {
        return;
    }
    voice(ui, app);
    local_model(ui, app);
    providers(ui, app);
    choices(ui, app);
    local(ui, app);
    actions(ui, app);
}

/// The one simple switch: where the AI runs (This PC / Online), the online backup, Test and voice on/off.
fn runs_on(ui: &mut Ui, app: &mut PimaxApp) {
    let local = app.core.ai_local_provider();
    let online = app.core.ai_online_provider();
    let is_online = app.core.ai_voice_online();
    card(ui, |ui| {
        let p = theme::pal(ui);
        ui.horizontal(|ui| {
            title(ui, "AI runs on");
            widgets::help(ui, "The voice control in VR switches by itself; the Work desk's watch has the same switch (AI button).");
        });
        let now = app.core.ai_runs_on();
        let local_on = app.core.ai.config.local_models_on();
        ui.label(RichText::new(match (now, local_on) {
            (RunsOn::Online, _) => "Online: Jev sorts the command, an action model acts (OpenRouter).".to_string(),
            (_, true) => format!("This PC: local models in Work ({}).", app.core.ai.config.preset().map(|p| p.name()).unwrap_or("chosen by hand")),
            (_, false) => "Offline commands only (local models off).".to_string(),
        }).size(13.0).color(p.text));
        if widgets::ibutton(ui, icon::ROBOT, "Local AI page", true).on_hover_text("Local models On / Off, Low / Medium / High, Online").clicked() {
            app.core.page = pimax_gui_core::app::Page::LocalAi;
        }
        if !is_online && online.is_some() {
            let mut b = app.core.ai.config.voice_online_backup.unwrap_or(true);
            if ui.checkbox(&mut b, "Use online if the local model is not running").changed() {
                app.core.ai_set_online_backup(b);
            }
        }
        let mode = app.core.ai.config.voice_ai;
        let mut on = mode != VoiceAi::Off;
        if widgets::toggle(ui, &mut on, "AI for voice commands", true).changed() {
            app.core.ai_set_voice_ai(if on { VoiceAi::Fallback } else { VoiceAi::Off });
        }
        let target = if is_online { online } else { local };
        if now == RunsOn::TwoStage {
            return;
        }
        match target {
            None => note(ui, "No provider of this kind is set up yet."),
            Some(name) => {
                ui.horizontal(|ui| {
                    if app.core.ai.testing.contains(&name) {
                        spinner(ui);
                    } else if widgets::ibutton(ui, icon::LIGHTNING, "Test", true).on_hover_text("Asks the AI one fixed question; nothing from this PC is sent").clicked() {
                        app.core.ai_test(&name);
                    }
                });
                match app.core.ai.tests.get(&name) {
                    Some(Ok(t)) => {
                        ui.label(RichText::new(format!("{} {t}", icon::CHECK_CIRCLE)).size(12.0).color(p.ok));
                    }
                    Some(Err(e)) => {
                        let e = pimax_gui_core::app::AppCore::ai_plain_error(e);
                        ui.add(egui::Label::new(RichText::new(format!("{} {e}", icon::X_CIRCLE)).size(12.0).color(p.fail)).wrap());
                    }
                    None => {}
                }
            }
        }
    });
}

/// Voice control's use of a model and the privacy switch.
fn voice(ui: &mut Ui, app: &mut PimaxApp) {
    card(ui, |ui| {
        title(ui, "Voice control with AI");
        let mode = app.core.ai.config.voice_ai;
        let mut on = mode != VoiceAi::Off;
        if widgets::toggle(ui, &mut on, "AI for voice commands", true).changed() {
            app.core.ai_set_voice_ai(if on { VoiceAi::Fallback } else { VoiceAi::Off });
        }
        if mode != VoiceAi::Off {
            let names = [VoiceAi::Fallback.label(), VoiceAi::Always.label()];
            let cur = usize::from(mode == VoiceAi::Always);
            if let Some(i) = widgets::selector(ui, icon::MICROPHONE, "When", &names, cur, true) {
                app.core.ai_set_voice_ai(if i == 1 { VoiceAi::Always } else { VoiceAi::Fallback });
            }
        }
        if mode != VoiceAi::Off {
            voice_provider(ui, app);
        }
        note(ui, match mode {
            VoiceAi::Off => "Voice commands use the offline matcher only: nothing you say leaves this PC.",
            VoiceAi::Fallback => "The offline matcher answers first; only a command it does not understand goes to the provider picked for voice control.",
            VoiceAi::Always => "Every \"Pimax ...\" command goes to the provider picked for voice control.",
        });
        // when a command waits for a spoken "yes" (works with or without the AI)
        let conf = app.core.ai.config.voice_confirm;
        let names = VoiceConfirm::ALL.map(|v| v.label());
        let cur = VoiceConfirm::ALL.iter().position(|v| *v == conf).unwrap_or(0);
        if let Some(i) = widgets::selector(ui, icon::MICROPHONE, "Confirm", &names, cur, true) {
            app.core.ai_set_voice_confirm(VoiceConfirm::ALL[i]);
        }
        note(ui, match conf {
            VoiceConfirm::None => "Every command runs at once, including close window, lock and typing into a terminal. Everything is logged.",
            VoiceConfirm::Dangerous => "Say \"yes\" to confirm lock, close window, and typing or Enter into a terminal.",
            VoiceConfirm::Always => "Say \"yes\" to confirm every voice command.",
        });
        let mut strip = app.core.ai.config.strip_titles;
        if widgets::switch_row(ui, &mut strip, "No window titles", Some("Privacy: window control sends the names of your apps and, unless this is on, their window titles (which can show a file or page name)."), true).changed() {
            app.core.ai_set_strip_titles(strip);
        }
        egui::CollapsingHeader::new("Advanced").id_salt("ai-advanced").show(ui, |ui| advanced(ui, app));
        // the voice command audit (pimax-voice audit, dry run): every command and natural phrasings
        let busy = app.core.busy();
        let report = app.core.voice_audit_report();
        ui.horizontal_wrapped(|ui| {
            if widgets::ibutton(ui, icon::LIGHTNING, "Check all voice commands", !busy)
                .on_hover_text("Runs every voice command and ~200 natural phrasings through the matcher and the model, without executing anything. A few minutes; between VR sessions.")
                .clicked()
            {
                app.core.voice_audit();
            }
            if report.is_file() && widgets::ibutton(ui, icon::CHECK_CIRCLE, "Open the last check", true).clicked() {
                app.core.open_folder(&report);
            }
        });
    });
}

/// The provider voice control uses (local / OpenRouter / others), its model, Test with latency and cost, and for a
/// remote one the two stages and what is sent (docs/AI.md 14).
fn voice_provider(ui: &mut Ui, app: &mut PimaxApp) {
    let p = theme::pal(ui);
    let names: Vec<String> = app.core.ai.config.providers.iter().map(|p| p.name.clone()).collect();
    if names.is_empty() {
        note(ui, "No provider yet: voice uses the local model on this PC (pimax-ai) when it runs. Add one below.");
        return;
    }
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let cur = app.core.ai.config.provider_for(Feature::Voice).and_then(|v| names.iter().position(|n| *n == v.name)).unwrap_or(0);
    if let Some(i) = widgets::selector(ui, icon::ROBOT, "Voice uses", &refs, cur, true) {
        let n = names[i].clone();
        app.core.ai_set_use(Feature::Voice, Some(&n));
    }
    let Ok(r) = app.core.ai_resolve(Feature::Voice) else {
        if let Err(e) = app.core.ai_resolve(Feature::Voice) {
            ui.label(RichText::new(e).size(12.0).color(p.warn));
        }
        return;
    };
    let name = r.provider.clone();
    ui.horizontal_wrapped(|ui| {
        let models = match &r.action_model {
            Some(a) => format!("intent {}, action {a}", r.model),
            None => r.model.clone(),
        };
        ui.label(RichText::new(format!("{models}  |  {}", if r.local() { "on this PC" } else { "remote" })).size(12.5).color(p.dim));
        if app.core.ai.testing.contains(&name) {
            spinner(ui);
        } else if widgets::ibutton(ui, icon::LIGHTNING, "Test", true).on_hover_text(format!("Sends \"{TEST_PROMPT}\" and shows the time (and the price, when the provider says)")).clicked() {
            app.core.ai_test(&name);
        }
        if widgets::ibutton(ui, icon::SLIDERS, "Model and key", true).on_hover_text("Edit this provider: model, key (masked, saved with mode 600)").clicked() {
            if let Some(pr) = app.core.ai.config.provider(&name) {
                app.ui.ai_draft = Some(AiDraft::edit(pr, app.core.ai.session_keys.get(&name)));
            }
        }
    });
    match app.core.ai.tests.get(&name) {
        Some(Ok(t)) => {
            ui.label(RichText::new(format!("{} {t}", icon::CHECK_CIRCLE)).size(12.0).color(p.ok));
        }
        Some(Err(e)) => {
            ui.add(egui::Label::new(RichText::new(format!("{} {e}", icon::X_CIRCLE)).size(12.0).color(p.fail)).wrap());
        }
        None => {}
    }
    if !r.local() {
        ui.horizontal_wrapped(|ui| {
            caption(ui, "Remote, two steps");
            widgets::help(ui, "Remote, in two steps: the intent model sorts the command (screens, apps, tabs, typing, ...) from your words and a one-line summary, then the action model gets only that kind's tools and the state they need, and acts. If it fails or takes longer than the timeout, the local model answers instead (when it runs), else the offline matcher.");
            ui.add_space(8.0);
            caption(ui, "What is sent");
            widgets::help(ui, "Sent after \"Max ...\", to the intent model: your words, the number of VR screens, the focused app's name, the side and preset. To the action model, only what that kind of command needs: your words, the focused app, and for screens and apps your VR screens with their apps (names and, unless the privacy switch is on, titles) and what you look at; for tabs and the browser your apps; for presets, passthrough, recording and captions the VR session state (and for undo your last 5 commands). Never sent: audio, screenshots, file contents, URLs, process ids.");
        });
    }
}

/// The optional voice settings (docs/AI.md 2): kept in ai.toml only when changed from the default.
fn advanced(ui: &mut Ui, app: &mut PimaxApp) {
    let c = &app.core.ai.config;
    let (mut timeout, mut gaze, mut log) = (c.voice_ai_timeout_s, c.voice_ai_gaze, c.voice_ai_log);
    let mut changed = false;
    ui.horizontal(|ui| {
        let mut custom = timeout.is_some();
        if ui.checkbox(&mut custom, "Answer timeout").on_hover_text("How long voice control waits for the model (default: 4 s local, 8 s remote)").changed() {
            timeout = custom.then_some(timeout.unwrap_or(4.0));
            changed = true;
        }
        if let Some(t) = timeout.as_mut() {
            if ui.add(egui::DragValue::new(t).range(1.0..=60.0).speed(0.1).suffix(" s")).changed() {
                changed = true;
            }
        } else {
            ui.label(RichText::new("default (4 s local, 8 s remote)").size(12.0).color(theme::pal(ui).dim));
        }
    });
    let mut g = gaze.unwrap_or(true);
    if ui.checkbox(&mut g, "Send what I look at with a voice request (gaze)").changed() {
        gaze = (!g).then_some(false);
        changed = true;
    }
    let mut l = log.unwrap_or(true);
    if ui.checkbox(&mut l, "Log AI requests (~/.local/state/pimax/logs/ai.log; keys are never logged)").changed() {
        log = (!l).then_some(false);
        changed = true;
    }
    if changed {
        app.core.ai_set_advanced(timeout, gaze, log);
    }
}

/// The local model on this PC (pimax-ai: llama.cpp with Qwen3.5).
fn local_model(ui: &mut Ui, app: &mut PimaxApp) {
    app.core.want_localai();
    let busy = app.core.busy();
    let status = app.core.localai.status.clone();
    let built = app.core.localai.bin.is_some();
    let mut act: Option<&'static str> = None;
    card(ui, |ui| {
        let p = theme::pal(ui);
        ui.horizontal_wrapped(|ui| {
            title(ui, "Local model on this PC");
            match &status {
                Some(Ok(s)) => {
                    chip(ui, if s.installed { "installed" } else { "not installed" }, if s.installed { pal(ui).ok } else { pal(ui).muted });
                    if let Some(r) = &s.running {
                        chip(ui, &format!("running: {}", r.mode), pal(ui).info);
                        chip(ui, if s.ready { "ready" } else { "loading" }, if s.ready { pal(ui).ok } else { pal(ui).warn });
                    }
                }
                Some(Err(_)) => chip(ui, "not available", pal(ui).muted),
                None => {
                    spinner(ui);
                }
            }
        });
        widgets::status(ui, "pimax-ai: Qwen3.5 with llama.cpp on 127.0.0.1:8080, on this PC.", p.faint);
        match &status {
            Some(Err(e)) => {
                ui.label(RichText::new(e).size(12.0).color(p.warn));
                if !built && widgets::ibutton(ui, icon::CODE, "Build pimax-ai", !busy).on_hover_text("cargo build --release -p pimax-ai").clicked() {
                    act = Some("build");
                }
            }
            Some(Ok(s)) => {
                if !s.installed {
                    if widgets::primary(ui, &format!("Install ({})", pimax_gui_core::localai::INSTALL_SIZE), !busy)
                        .on_hover_text("pimax-ai install: the llama.cpp runtime and both models into ~/.local/share/pimax/ai (resumes when run again)")
                        .clicked()
                    {
                        act = Some("install");
                    }
                } else if let Some(r) = &s.running {
                    ui.label(RichText::new(format!("{} (pid {}), {}", r.model, r.pid, r.url)).size(12.0).color(p.dim));
                    if let (Some(used), Some(budget)) = (s.gpu_mib, s.gpu_budget_mib) {
                        let k = (used as f32 / budget.max(1) as f32).min(1.0);
                        ui.add(egui::ProgressBar::new(k).desired_width(ui.available_width().min(420.0)).text(format!("VRAM {used} / {budget} MiB")));
                    } else if s.on_gpu() {
                        ui.label(RichText::new("VRAM: not reported (nvidia-smi)").size(12.0).color(p.faint));
                    }
                    ui.horizontal_wrapped(|ui| {
                        if widgets::ibutton(ui, icon::STOP, "Stop", !busy).on_hover_text("pimax-ai stop").clicked() {
                            act = Some("stop");
                        }
                    });
                } else {
                    let warn = app.core.localai_gpu_warning();
                    ui.horizontal_wrapped(|ui| {
                        if widgets::ibutton(ui, icon::PLAY, "Start on the GPU", !busy).on_hover_text("pimax-ai serve: all layers on the GPU, about 3.5 GiB of VRAM (budget 4 GiB)").clicked() {
                            act = Some("gpu");
                        }
                        if widgets::ibutton(ui, icon::PLAY, "Start on the CPU", !busy).on_hover_text("pimax-ai serve --cpu: no GPU at all (the 2B model)").clicked() {
                            act = Some("cpu");
                        }
                    });
                    if let Some(w) = warn {
                        ui.label(RichText::new(w).size(12.0).color(p.warn));
                    }
                }
                match app.core.localai_provider() {
                    Some(n) => {
                        ui.label(RichText::new(format!("Provider {n} points at it.")).size(12.0).color(p.dim));
                    }
                    None => {
                        if widgets::primary(ui, "Use local model for voice", true)
                            .on_hover_text("Adds the provider (openai-compatible, http://127.0.0.1:8080/v1, pimax-local) and picks it for voice control")
                            .clicked()
                        {
                            act = Some("use");
                        }
                    }
                }
            }
            None => {}
        }
    });
    match act {
        Some("install") => app.core.localai_install(),
        Some("gpu") => app.core.localai_start(true),
        Some("cpu") => app.core.localai_start(false),
        Some("stop") => app.core.localai_stop(),
        Some("build") => app.core.localai_build(),
        Some("use") => {
            if let Err(e) = app.core.localai_use() {
                app.core.message = e;
            }
        }
        _ => {}
    }
}

/// The last AI actions of voice control (`~/.local/state/pimax/logs/ai.log`).
fn actions(ui: &mut Ui, app: &mut PimaxApp) {
    app.core.want_ai_log();
    card(ui, |ui| {
        let p = theme::pal(ui);
        title(ui, "Last AI actions");
        if app.core.ai.log.is_empty() {
            note(ui, "None yet (~/.local/state/pimax/logs/ai.log).");
        }
        for l in app.core.ai.log.iter().take(12) {
            ui.add(egui::Label::new(RichText::new(l).monospace().size(11.5).color(p.dim)).truncate());
        }
    });
}

fn providers(ui: &mut Ui, app: &mut PimaxApp) {
    let list = app.core.ai.config.providers.clone();
    let default = app.core.ai.config.default.clone();
    let mut edit = None;
    let mut remove = None;
    let mut test = None;
    card(ui, |ui| {
        title(ui, "Providers");
        if list.is_empty() {
            note(ui, "None yet: add Claude or OpenAI with a key, or a local model (below).");
        }
        for pr in &list {
            let (ks, ok) = key_status(&app.core.ai, pr, &|k| app.core.env_var(k));
            inset(ui, |ui| {
                let p = theme::pal(ui);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&pr.name).strong().size(15.0));
                    chip(ui, pr.kind.label(), pal(ui).info);
                    if default.as_deref() == Some(pr.name.as_str()) {
                        chip(ui, "default", pal(ui).ok);
                    }
                });
                let model = CLAUDE_MODELS.iter().find(|(id, _)| *id == pr.model).map_or(pr.model.as_str(), |(_, n)| n);
                ui.label(RichText::new(format!("{}  |  {}", if model.is_empty() { "no model" } else { model }, pr.base().unwrap_or("no base URL"))).size(12.5).color(p.dim));
                ui.label(RichText::new(ks).size(12.0).color(if ok { p.ok } else { p.warn }));
                match app.core.ai.tests.get(&pr.name) {
                    Some(Ok(t)) => {
                        ui.label(RichText::new(format!("{} {t}", icon::CHECK_CIRCLE)).size(12.0).color(p.ok));
                    }
                    Some(Err(e)) => {
                        ui.add(egui::Label::new(RichText::new(format!("{} {e}", icon::X_CIRCLE)).size(12.0).color(p.fail)).wrap());
                    }
                    None => {}
                }
                ui.horizontal_wrapped(|ui| {
                    if app.core.ai.testing.contains(&pr.name) {
                        spinner(ui);
                        ui.label(RichText::new("Testing...").size(12.0).color(p.dim));
                    } else if widgets::ibutton(ui, icon::LIGHTNING, "Test connection", true)
                        .on_hover_text(format!("Sends \"{TEST_PROMPT}\" to {} (nothing else)", pr.base().unwrap_or("?")))
                        .clicked()
                    {
                        test = Some(pr.name.clone());
                    }
                    if widgets::ibutton(ui, icon::SLIDERS, "Edit", true).clicked() {
                        edit = Some(pr.name.clone());
                    }
                    if widgets::ibutton(ui, icon::X_CIRCLE, "Remove", true).clicked() {
                        remove = Some(pr.name.clone());
                    }
                });
            });
            ui.add_space(4.0);
        }
        if app.ui.ai_draft.is_none() {
            ui.horizontal_wrapped(|ui| {
                for (kind, label, base) in [(Kind::Anthropic, "Add Claude", "claude"), (Kind::OpenAi, "Add OpenAI", "openai"), (Kind::OpenAiCompatible, "Add OpenRouter", "openrouter"), (Kind::OpenAiCompatible, "Add a local or other model", "local")] {
                    if widgets::ibutton(ui, icon::SPARKLE, label, true).clicked() {
                        let name = unique(app, base);
                        let mut d = AiDraft::new(kind, &name);
                        if base == "openrouter" {
                            d.base_url = pimax_config::ai::OPENROUTER_BASE.into();
                            d.model = pimax_config::ai::OPENROUTER_VOICE_MODEL.into();
                            d.action_model = pimax_config::ai::OPENROUTER_ACTION_MODELS[0].into();
                        }
                        app.ui.ai_draft = Some(d);
                    }
                }
            });
        }
        editor(ui, app);
    });
    if let Some(n) = test {
        app.core.ai_test(&n);
    }
    if let Some(n) = edit {
        if let Some(pr) = app.core.ai.config.provider(&n) {
            app.ui.ai_draft = Some(AiDraft::edit(pr, app.core.ai.session_keys.get(&n)));
        }
    }
    if let Some(n) = remove {
        app.core.ai_remove(&n);
    }
}

fn editor(ui: &mut Ui, app: &mut PimaxApp) {
    let Some(mut d) = app.ui.ai_draft.take() else { return };
    let found: Vec<String> = app.core.ai.found.iter().map(|b| b.model.clone()).collect();
    let mut save = false;
    let mut cancel = false;
    ui.add_space(6.0);
    inset(ui, |ui| {
        let p = theme::pal(ui);
        title(ui, if d.old.is_some() { "Edit provider" } else { "New provider" });
        let kinds: Vec<&str> = Kind::ALL.iter().map(|k| k.label()).collect();
        if let Some(i) = widgets::selector(ui, icon::ROBOT, "Kind", &kinds, d.kind, true) {
            if i != d.kind {
                let fresh = AiDraft::new(Kind::ALL[i], &d.name);
                d.kind = i;
                d.model = fresh.model;
                d.base_url = fresh.base_url;
            }
        }
        let kind = Kind::ALL[d.kind];
        egui::Grid::new("ai-editor").num_columns(2).spacing([14.0, 8.0]).show(ui, |ui| {
            ui.label(RichText::new("Name").color(p.dim));
            ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(220.0));
            ui.end_row();
            ui.label(RichText::new(if d.action_model.trim().is_empty() { "Model" } else { "Intent model" }).color(p.dim))
                .on_hover_text("Voice control: this model sorts a command into a kind (screens, apps, tabs, ...)");
            if kind == Kind::Anthropic {
                let names: Vec<&str> = CLAUDE_MODELS.iter().map(|(_, n)| *n).collect();
                let cur = CLAUDE_MODELS.iter().position(|(id, _)| *id == d.model).unwrap_or(0);
                ui.horizontal(|ui| {
                    for (i, n) in names.iter().enumerate() {
                        if ui.selectable_label(i == cur, *n).clicked() {
                            d.model = CLAUDE_MODELS[i].0.to_string();
                        }
                    }
                });
                if !CLAUDE_MODELS.iter().any(|(id, _)| *id == d.model) {
                    d.model = CLAUDE_MODELS[0].0.to_string();
                }
            } else {
                ui.add(egui::TextEdit::singleline(&mut d.model).desired_width(260.0).hint_text(if kind == Kind::OpenAi { "the model name, as OpenAI lists it" } else { "e.g. qwen3:8b" }));
            }
            ui.end_row();
            if kind != Kind::Anthropic {
                ui.label(RichText::new("Action model").color(p.dim))
                    .on_hover_text("Voice control: this model acts on the command with that kind's tools (empty: the model above does both)");
                ui.vertical(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut d.action_model).desired_width(260.0).hint_text("optional, e.g. openai/gpt-6-luna"));
                    if d.base_url.starts_with("https://openrouter.ai/") {
                        ui.horizontal_wrapped(|ui| {
                            for m in pimax_config::ai::OPENROUTER_ACTION_MODELS {
                                if ui.selectable_label(d.action_model == m, m).clicked() {
                                    d.action_model = m.to_string();
                                }
                            }
                        });
                    }
                });
                ui.end_row();
            }
            ui.label(RichText::new("Base URL").color(p.dim));
            ui.add(egui::TextEdit::singleline(&mut d.base_url).desired_width(320.0).hint_text(kind.default_base().unwrap_or("http://127.0.0.1:11434/v1")));
            ui.end_row();
            if kind.needs_key() || !d.key.is_empty() {
                ui.label(RichText::new("Key").color(p.dim));
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut d.key).password(!d.show).desired_width(280.0).hint_text(if kind == Kind::Anthropic { "sk-ant-..." } else { "the key or token" }));
                    let label = if d.show { "Hide" } else { "Show" };
                    if widgets::ismall(ui, if d.show { icon::EYE_SLASH } else { icon::EYE }, label).clicked() {
                        d.show = !d.show;
                    }
                });
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut d.save_key, "Save the key in ai.toml (untick: keep it for this session only)");
                ui.end_row();
            }
            ui.label(RichText::new("Or from").color(p.dim));
            ui.add(egui::TextEdit::singleline(&mut d.key_env).desired_width(220.0).hint_text(kind.usual_env().unwrap_or("an environment variable")));
            ui.end_row();
        });
        if kind == Kind::OpenAiCompatible && !found.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Found here:").size(12.0).color(p.dim));
                for m in &found {
                    if ui.small_button(m).clicked() {
                        d.model = m.clone();
                    }
                }
            });
        }
        if !d.error.is_empty() {
            ui.label(RichText::new(&d.error).color(p.fail));
        }
        ui.horizontal(|ui| {
            if widgets::primary(ui, "Save", true).clicked() {
                save = true;
            }
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
        });
    });
    if save {
        let kind = Kind::ALL[d.kind];
        let mut pr = d.old.as_deref().and_then(|o| app.core.ai.config.provider(o)).cloned().unwrap_or_else(|| Provider::new(&d.name, kind));
        pr.name = d.name.clone();
        pr.kind = kind;
        pr.model = d.model.clone();
        pr.action_model = Some(d.action_model.trim().to_string()).filter(|m| !m.is_empty() && kind != Kind::Anthropic);
        pr.base_url = Some(d.base_url.clone());
        pr.api_key_env = Some(d.key_env.clone());
        let key = Secret::new(&d.key);
        let choice = if key.is_empty() {
            KeyChoice::None
        } else if d.save_key {
            KeyChoice::Save(key)
        } else {
            KeyChoice::Session(key)
        };
        match app.core.ai_put(d.old.as_deref(), pr, choice) {
            Ok(()) => {
                d.wipe();
                return;
            }
            Err(e) => d.error = e,
        }
    }
    if cancel {
        d.wipe();
        return;
    }
    app.ui.ai_draft = Some(d);
}

fn choices(ui: &mut Ui, app: &mut PimaxApp) {
    let names: Vec<String> = app.core.ai.config.providers.iter().map(|p| p.name.clone()).collect();
    if names.is_empty() {
        return;
    }
    card(ui, |ui| {
        title(ui, "Which provider for what");
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let cur = app.core.ai.config.default.as_deref().and_then(|d| names.iter().position(|n| n == d)).unwrap_or(0);
        if let Some(i) = widgets::selector(ui, icon::ROBOT, "Default", &refs, cur, true) {
            let n = names[i].clone();
            app.core.ai_set_default(&n);
        }
        let mut opts: Vec<&str> = vec!["the default"];
        opts.extend(refs.iter().copied());
        for (f, ic) in [(Feature::Voice, icon::MICROPHONE), (Feature::Autofix, icon::WRENCH)] {
            let cur = app.core.ai.config.uses.get(&f).and_then(|u| names.iter().position(|n| n == u)).map_or(0, |i| i + 1);
            if let Some(i) = widgets::selector(ui, ic, f.label(), &opts, cur, true) {
                let n = (i > 0).then(|| names[i - 1].clone());
                app.core.ai_set_use(f, n.as_deref());
            }
            let line = match app.core.ai_resolve(f) {
                Ok(r) => (format!("{}: {} ({}), key {}", f.label(), r.provider, r.model, match r.key_source {
                    pimax_config::ai::KeySource::File => "from ai.toml".to_string(),
                    pimax_config::ai::KeySource::Env(e) => format!("from ${e}"),
                    pimax_config::ai::KeySource::Session => "for this session only".to_string(),
                    pimax_config::ai::KeySource::None => "not needed".to_string(),
                }), true),
                Err(e) => (format!("{}: {e}", f.label()), false),
            };
            let p = theme::pal(ui);
            ui.label(RichText::new(line.0).size(12.0).color(if line.1 { p.dim } else { p.warn }));
        }
        note(ui, "Session-only keys work for Autofix (it runs in this app); voice control reads ai.toml and the environment only.");
    });
}

fn local(ui: &mut Ui, app: &mut PimaxApp) {
    let mut add = None;
    card(ui, |ui| {
        let p = theme::pal(ui);
        ui.horizontal(|ui| {
            title(ui, "Local models on this PC");
            if app.core.ai.probing {
                spinner(ui);
            } else if widgets::ismall(ui, icon::ARROWS_CLOCKWISE, "Look again").on_hover_text("Asks 127.0.0.1:11434 (ollama) and 127.0.0.1:8080 (llama-server) only").clicked() {
                app.core.ai_probe();
            }
        });
        if app.core.ai.probed && app.core.ai.found.is_empty() {
            note(ui, "None answered on 127.0.0.1:11434 (ollama) or 127.0.0.1:8080 (llama-server). Start one (for example `ollama serve` and `ollama pull qwen3:8b`), then Look again; LM Studio and others can be added by hand with their base URL.");
        }
        for (i, b) in app.core.ai.found.iter().enumerate() {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("{} {}", icon::DESKTOP, b.label())).color(p.text));
                if widgets::ismall(ui, icon::SPARKLE, "Add").on_hover_text("Add it as a provider (no key, stays on this PC)").clicked() {
                    add = Some(i);
                }
            });
        }
    });
    if let Some(i) = add {
        if let Err(e) = app.core.ai_add_found(i) {
            app.core.message = e;
        }
    }
}
