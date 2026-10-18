//! AI (the one AI tab): which AI answers voice commands (Claude CLI by default, a local model, or online with a key),
//! the Claude CLI card (status, model, Test), the voice settings, the local model section (pages/localai.rs), and the
//! providers voice control and Autofix use (`~/.config/omarchyvr/ai.toml`, shared with omarchyvr-voice): add, edit,
//! remove, a masked key with show/hide (saved, or kept for this session only), Test connection (the one network call
//! here, on click: a fixed one-line prompt, no data from this PC), the default and per-feature provider, and the
//! local models found on 127.0.0.1.
use crate::app::PimaxApp;
use crate::theme::{self, icon};
use crate::widgets::{self, card, caption, chip, inset, note, pal, spinner, title};
use eframe::egui::{self, RichText, Ui};
use omarchyvr_config::ai::{Feature, Kind, Provider, Secret, VoiceAi, VoiceConfirm, VoiceListen, CLAUDE_CLI_MODELS, CLAUDE_CLI_NAME, CLAUDE_MODELS};
use omarchyvr_config::claude_cli::ClaudeCliStatus;
use omarchyvr_gui_core::ai::{curl_capable, key_status, KeyChoice, VoiceBrain, TEST_PROMPT};

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
    /// The Claude CLI's command path (empty: `claude`).
    pub command: String,
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
            command: String::new(),
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
            command: p.command.clone().unwrap_or_default(),
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

/// A block inside a section (no frame of its own).
fn plain<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    add(ui)
}

/// A collapsible card: the header always shows, the body opens on a click (`open`: the first state).
fn fold(ui: &mut Ui, id: &str, title_text: &str, open: bool, chips: impl FnOnce(&mut Ui), add: impl FnOnce(&mut Ui)) {
    card(ui, |ui| {
        let p = theme::pal(ui);
        let head = RichText::new(title_text).size(15.0).strong().color(p.text);
        egui::CollapsingHeader::new(head).id_salt(("ai-fold", id)).default_open(open).show(ui, |ui| {
            ui.horizontal_wrapped(chips);
            add(ui);
        });
    });
    ui.add_space(8.0);
}

/// Draw the page.
pub fn show(ui: &mut Ui, app: &mut PimaxApp) {
    app.core.want_ai();
    let p = theme::pal(ui);
    card(ui, |ui| {
        ui.horizontal(|ui| {
            title(ui, "AI");
            widgets::help(ui, "The language models voice control (\"Max, ...\") and Autofix use: Claude through the Claude Code you are logged into (default, no key), a model running on this PC, or an online provider with a key.");
            if app.core.demo {
                chip(ui, "demo: nothing is read, written or sent", pal(ui).info);
            }
        });
        ui.horizontal(|ui| {
            caption(ui, "What is sent where");
            widgets::help(ui, &format!("This page sends nothing by itself. The Test buttons send one fixed line (\"{TEST_PROMPT}\"), nothing from this PC. Autofix sends its redacted bundle (Setup > Autofix shows it) only when you press Send. Voice control sends the words you say, and the screen state, to the AI picked below. A local model keeps everything on this PC.\n\nKeys are saved in ~/.config/omarchyvr/ai.toml, readable only by you (mode 600), or kept for this session only when you untick \"Save the key\" (then voice control, a separate program, cannot use them). The Claude CLI needs no key."));
        });
        if let Some(e) = &app.core.ai.error {
            ui.label(RichText::new(format!("{e}: fix or remove the file; the app does not overwrite it.")).color(p.fail));
        }
        for w in &app.core.ai.config.warnings {
            ui.label(RichText::new(w).size(12.0).color(p.warn_text));
        }
        if !app.core.ai.saved.is_empty() {
            ui.label(RichText::new(&app.core.ai.saved).size(11.5).color(p.faint));
        }
        // the questions of the first Work press (Lara, the brain, speech), to change the answers
        ui.horizontal_wrapped(|ui| {
            if widgets::ghost(ui, icon::SLIDERS, "Work questions", !app.core.busy()).on_hover_text("The voice assistant (Lara and who answers) and speech to text, as the first Work asked them; downloads what a new answer needs").clicked() {
                app.core.open_work_setup(false);
            }
            if !app.core.work_setup_done() {
                ui.label(RichText::new("not answered yet: the next Work asks them").size(11.5).color(p.dim));
            }
        });
    });
    if !app.core.ai.loaded {
        card(ui, |ui| {
            ui.horizontal(|ui| {
                spinner(ui);
                ui.label("Reading ~/.config/omarchyvr/ai.toml...");
            });
        });
        return;
    }
    // opened again (the page was not drawn for a moment): look at the Claude CLI again
    let now = ui.ctx().input(|i| i.time);
    if now - app.ui.ai_seen > 1.0 {
        app.core.claude_look();
    }
    app.ui.ai_seen = now;
    app.core.want_claude_status();
    let brain = app.core.ai_voice_brain();
    let dev = app.core.state.dev_mode;
    brain_card(ui, app, brain);
    let ok = app.core.ai.claude_status.as_ref().is_some_and(ClaudeCliStatus::ok);
    let has_status = app.core.ai.claude_status.is_some();
    fold(ui, "claude", "Claude (CLI)", brain == VoiceBrain::Claude, |ui| {
        if brain == VoiceBrain::Claude {
            chip(ui, "answers voice commands", pal(ui).ok);
        }
        if has_status {
            chip(ui, if ok { "found" } else { "not working" }, if ok { pal(ui).ok } else { pal(ui).warn });
        }
    }, |ui| claude_card(ui, app));
    let voice_open = app.ui.ai_voice_open;
    fold(ui, "voice", "Voice settings", voice_open, |_| {}, |ui| voice(ui, app, brain));
    fold(ui, "local", "Local model on this PC", brain == VoiceBrain::Local, |ui| {
        if brain == VoiceBrain::Local {
            chip(ui, "answers voice commands", pal(ui).ok);
        }
    }, |ui| {
        crate::pages::localai::body(ui, app);
        if dev {
            ui.add_space(8.0);
            local_model(ui, app);
        }
    });
    fold(ui, "online", "Online providers", brain == VoiceBrain::Online, |ui| {
        if brain == VoiceBrain::Online {
            chip(ui, "answers voice commands", pal(ui).ok);
        }
    }, |ui| {
        providers(ui, app);
        ui.add_space(8.0);
        choices(ui, app);
        if dev {
            ui.add_space(8.0);
            local(ui, app);
        }
    });
    if dev {
        actions(ui, app);
    }
}

/// "Max's brain": which AI answers voice commands.
fn brain_card(ui: &mut Ui, app: &mut PimaxApp, brain: VoiceBrain) {
    card(ui, |ui| {
        let p = theme::pal(ui);
        ui.horizontal(|ui| {
            title(ui, "Max's brain");
            widgets::help(ui, "Max is the voice assistant (\"Max, move this to screen 2\"). Exact phrases run on this PC without any AI; this choice is who understands the rest. Claude (CLI) uses the Claude Code you are logged into: no key. A local model keeps everything on this PC (Work only). Online uses a provider with your key.\n\nListening: how long Max waits after you stop talking. Long (default) lets you say several things in one command with short pauses between them.");
        });
        let names: Vec<&str> = VoiceBrain::ALL.iter().map(|b| b.label()).collect();
        let cur = VoiceBrain::ALL.iter().position(|b| *b == brain).unwrap_or(0);
        if let Some(i) = widgets::seg_row(ui, "Voice commands use", None, &names, cur, !app.core.busy()) {
            app.core.ai_set_voice_brain(VoiceBrain::ALL[i]);
        }
        let line = match brain {
            VoiceBrain::Claude => "Claude (CLI): each command starts a fresh Claude with only your words and the screen state. No key.".to_string(),
            VoiceBrain::Local => format!("Local model on this PC ({}): nothing leaves this PC.", app.core.ai.config.preset().map(|p| p.name()).unwrap_or("chosen by hand")),
            VoiceBrain::Online => match app.core.ai_resolve(Feature::Voice) {
                Ok(r) => format!("Online: {} ({}). Your words and the screen state are sent.", r.provider, r.model),
                Err(e) => e,
            },
        };
        ui.add(egui::Label::new(RichText::new(line).size(13.0).color(p.text)).wrap());
        if brain != VoiceBrain::Online && app.core.ai_keyed_online_provider().is_none() {
            note(ui, "Online needs a provider with a key: add one under Online providers.");
        }
        if brain == VoiceBrain::Claude && app.core.ai.claude_status.as_ref().is_some_and(|s| !s.ok()) {
            ui.add(egui::Label::new(RichText::new("The Claude CLI is not working: voice commands the offline matcher does not know will not be answered. See the Claude (CLI) card.").size(12.0).color(p.warn_text)).wrap());
        }
    });
    ui.add_space(8.0);
}

/// The Claude CLI: status, model, Test.
fn claude_card(ui: &mut Ui, app: &mut PimaxApp) {
    let p = theme::pal(ui);
    ui.horizontal_wrapped(|ui| {
        match &app.core.ai.claude_status {
            None => {
                spinner(ui);
                ui.label(RichText::new("Looking for Claude Code...").size(12.5).color(p.dim));
            }
            Some(st) => {
                let (mark, color) = if st.ok() { (icon::CHECK_CIRCLE, p.ok) } else { (icon::X_CIRCLE, p.warn) };
                ui.add(egui::Label::new(RichText::new(format!("{mark} {}", st.plain())).size(12.5).color(color)).wrap());
            }
        }
        if app.core.ai.claude_checking {
            spinner(ui);
        } else if widgets::ismall(ui, icon::ARROWS_CLOCKWISE, "Look again").on_hover_text("Runs claude --version again").clicked() {
            app.core.claude_look();
        }
    });
    if matches!(app.core.ai.claude_status, Some(ClaudeCliStatus::NotFound)) {
        note(ui, "Claude Code is not installed on this PC. Install Claude Code and log in once, then press Look again.");
    }
    // the model
    let model = app.core.ai.config.claude_cli_provider().model.clone();
    let names: Vec<&str> = [("haiku", "Haiku (fast, default)"), ("sonnet", "Sonnet (more capable)")].iter().map(|m| m.1).collect();
    let cur = CLAUDE_CLI_MODELS.iter().position(|(id, _)| *id == model).unwrap_or(0);
    if let Some(i) = widgets::seg_row(ui, "Model", Some("Haiku answers fastest and is enough for voice commands. Sonnet is slower and better at long, multi-step requests."), &names, cur, true) {
        app.core.ai_set_claude_model(CLAUDE_CLI_MODELS[i].0);
    }
    // Test
    let ready = app.core.ai.claude_status.as_ref().is_some_and(ClaudeCliStatus::ok);
    ui.horizontal_wrapped(|ui| {
        if app.core.ai.claude_testing {
            spinner(ui);
            ui.label(RichText::new("Testing...").size(12.0).color(p.dim));
        } else if widgets::ibutton(ui, icon::LIGHTNING, "Test", ready).on_hover_text("Asks Claude one fixed question; nothing from this PC is sent").clicked() {
            app.core.claude_test_run();
        }
        let need_login = matches!(&app.core.ai.claude_test, Some(Err(e)) if e.to_ascii_lowercase().contains("log"));
        if ready && need_login && widgets::ibutton(ui, icon::ARROW_SQUARE_OUT, "Open Claude login", true).on_hover_text("Opens a terminal with Claude Code: log in there once").clicked() {
            app.core.claude_login();
        }
    });
    match &app.core.ai.claude_test {
        Some(Ok(t)) => {
            ui.add(egui::Label::new(RichText::new(format!("{} {t}", icon::CHECK_CIRCLE)).size(12.0).color(p.ok)).wrap());
        }
        Some(Err(e)) => {
            ui.add(egui::Label::new(RichText::new(format!("{} {e}", icon::X_CIRCLE)).size(12.0).color(p.fail)).wrap());
        }
        None => {}
    }
    note(ui, "Uses the Claude Code you are logged into: no key. Each command starts a fresh Claude with only the request and the screen state.");
}

/// Voice control's use of a model and the privacy switch.
fn voice(ui: &mut Ui, app: &mut PimaxApp, brain: VoiceBrain) {
    {
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
        if mode != VoiceAi::Off && brain == VoiceBrain::Online {
            voice_provider(ui, app);
        }
        note(ui, match mode {
            VoiceAi::Off => "Voice commands use the offline matcher only: nothing you say leaves this PC.",
            VoiceAi::Fallback => "The offline matcher answers first; only a command it does not understand goes to the AI picked for voice control.",
            VoiceAi::Always => "Every \"Max ...\" command goes to the AI picked for voice control.",
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
        // how long the listener waits after you stop talking (read when voice starts)
        let listen = app.core.ai.config.voice_listen;
        let names = VoiceListen::ALL.map(|v| v.label());
        let cur = VoiceListen::ALL.iter().position(|v| *v == listen).unwrap_or(1);
        if let Some(i) = widgets::selector(ui, icon::MICROPHONE, "Listening", &names, cur, true) {
            app.core.ai_set_voice_listen(VoiceListen::ALL[i]);
        }
        note(ui, "How long Max waits after you stop talking. Longer lets you say several things in one command; commands start a little later. Applies the next time voice starts.");
        let mut strip = app.core.ai.config.strip_titles;
        if widgets::switch_row(ui, &mut strip, "No window titles", Some("Privacy: window control sends the names of your apps and, unless this is on, their window titles (which can show a file or page name)."), true).changed() {
            app.core.ai_set_strip_titles(strip);
        }
        egui::CollapsingHeader::new("Advanced").id_salt("ai-advanced").show(ui, |ui| advanced(ui, app, brain));
        // the voice command audit (omarchyvr-voice audit, dry run): every command and natural phrasings
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
    }
}

/// The provider voice control uses (local / OpenRouter / others), its model, Test with latency and cost, and for a
/// remote one the two stages and what is sent (docs/AI.md 14).
fn voice_provider(ui: &mut Ui, app: &mut PimaxApp) {
    let p = theme::pal(ui);
    let names: Vec<String> = app.core.ai.config.providers.iter().filter(|p| curl_capable(p)).map(|p| p.name.clone()).collect();
    if names.is_empty() {
        note(ui, "No provider yet: voice uses the local model on this PC (omarchyvr-ai) when it runs. Add one below.");
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
            ui.label(RichText::new(e).size(12.0).color(p.warn_text));
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
fn advanced(ui: &mut Ui, app: &mut PimaxApp, brain: VoiceBrain) {
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
    if ui.checkbox(&mut l, "Log AI requests (~/.local/state/omarchyvr/logs/ai.log; keys are never logged)").changed() {
        log = (!l).then_some(false);
        changed = true;
    }
    if changed {
        app.core.ai_set_advanced(timeout, gaze, log);
    }
    if brain == VoiceBrain::Claude {
        let mut warm = app.core.ai.config.voice_ai_prewarm.unwrap_or(true);
        if widgets::switch_row(ui, &mut warm, "Start Claude ahead (faster)", Some("Keeps one Claude started and waiting, so the next voice command answers sooner. It sends nothing until you speak. Off: Claude starts when you give a command."), true).changed() {
            app.core.ai_set_prewarm(warm);
        }
    }
}

/// The local model on this PC (omarchyvr-ai: llama.cpp with Qwen3.5).
fn local_model(ui: &mut Ui, app: &mut PimaxApp) {
    app.core.want_localai();
    let busy = app.core.busy();
    let status = app.core.localai.status.clone();
    let built = app.core.localai.bin.is_some();
    let mut act: Option<&'static str> = None;
    plain(ui, |ui| {
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
        widgets::status(ui, "omarchyvr-ai: Qwen3.5 with llama.cpp on 127.0.0.1:8080, on this PC.", p.faint);
        match &status {
            Some(Err(e)) => {
                ui.label(RichText::new(e).size(12.0).color(p.warn_text));
                if !built && widgets::ibutton(ui, icon::CODE, "Repair omarchyvr-ai", !busy).on_hover_text("Rebuilds omarchyvr-ai from this install's source (a few minutes)").clicked() {
                    act = Some("build");
                }
            }
            Some(Ok(s)) => {
                if !s.installed {
                    if widgets::primary(ui, &format!("Install ({})", omarchyvr_gui_core::localai::INSTALL_SIZE), !busy)
                        .on_hover_text("omarchyvr-ai install: the llama.cpp runtime and both models into ~/.local/share/omarchyvr/ai (resumes when run again)")
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
                        if widgets::ibutton(ui, icon::STOP, "Stop", !busy).on_hover_text("omarchyvr-ai stop").clicked() {
                            act = Some("stop");
                        }
                    });
                } else {
                    let warn = app.core.localai_gpu_warning();
                    ui.horizontal_wrapped(|ui| {
                        if widgets::ibutton(ui, icon::PLAY, "Start on the GPU", !busy).on_hover_text("omarchyvr-ai serve: all layers on the GPU, about 3.5 GiB of VRAM (budget 4 GiB)").clicked() {
                            act = Some("gpu");
                        }
                        if widgets::ibutton(ui, icon::PLAY, "Start on the CPU", !busy).on_hover_text("omarchyvr-ai serve --cpu: no GPU at all (the 2B model)").clicked() {
                            act = Some("cpu");
                        }
                    });
                    if let Some(w) = warn {
                        ui.label(RichText::new(w).size(12.0).color(p.warn_text));
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

/// The last AI actions of voice control (`~/.local/state/omarchyvr/logs/ai.log`).
fn actions(ui: &mut Ui, app: &mut PimaxApp) {
    app.core.want_ai_log();
    card(ui, |ui| {
        let p = theme::pal(ui);
        title(ui, "Last AI actions");
        if app.core.ai.log.is_empty() {
            note(ui, "None yet (~/.local/state/omarchyvr/logs/ai.log).");
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
    plain(ui, |ui| {
        title(ui, "Providers");
        if list.is_empty() {
            note(ui, "None yet: add Claude or OpenAI with a key, or a local model (below). The Claude CLI needs no entry: see the Claude (CLI) card.");
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
                let model = CLAUDE_CLI_MODELS.iter().find(|(id, _)| *id == pr.model).map_or(model, |(_, n)| n);
                let place = if pr.kind == Kind::ClaudeCli { pr.cli_command() } else { pr.base().unwrap_or("no base URL") };
                ui.label(RichText::new(format!("{}  |  {}", if model.is_empty() { "no model" } else { model }, place)).size(12.5).color(p.dim));
                ui.label(RichText::new(ks).size(12.0).color(if ok { p.ok } else { p.warn }));
                let cli = pr.kind == Kind::ClaudeCli;
                let test_now = if cli { app.core.ai.claude_test.clone() } else { app.core.ai.tests.get(&pr.name).cloned() };
                match test_now.as_ref() {
                    Some(Ok(t)) => {
                        ui.label(RichText::new(format!("{} {t}", icon::CHECK_CIRCLE)).size(12.0).color(p.ok));
                    }
                    Some(Err(e)) => {
                        ui.add(egui::Label::new(RichText::new(format!("{} {e}", icon::X_CIRCLE)).size(12.0).color(p.fail)).wrap());
                    }
                    None => {}
                }
                ui.horizontal_wrapped(|ui| {
                    if app.core.ai.testing.contains(&pr.name) || (cli && app.core.ai.claude_testing) {
                        spinner(ui);
                        ui.label(RichText::new("Testing...").size(12.0).color(p.dim));
                    } else if widgets::ibutton(ui, icon::LIGHTNING, "Test connection", true)
                        .on_hover_text(format!("Sends \"{TEST_PROMPT}\" to {} (nothing else)", if cli { "Claude" } else { pr.base().unwrap_or("?") }))
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
                let has_cli = list.iter().any(|p| p.kind == Kind::ClaudeCli);
                for (kind, label, base) in [(Kind::Anthropic, "Add Claude (API key)", "claude"), (Kind::OpenAi, "Add OpenAI", "openai"), (Kind::OpenAiCompatible, "Add OpenRouter", "openrouter"), (Kind::OpenAiCompatible, "Add a local or other model", "local"), (Kind::ClaudeCli, "Add Claude (CLI, no key)", CLAUDE_CLI_NAME)] {
                    if kind == Kind::ClaudeCli && has_cli {
                        continue;
                    }
                    if widgets::ibutton(ui, icon::SPARKLE, label, true).clicked() {
                        let name = if kind == Kind::ClaudeCli { base.to_string() } else { unique(app, base) };
                        let mut d = AiDraft::new(kind, &name);
                        if base == "openrouter" {
                            d.base_url = omarchyvr_config::ai::OPENROUTER_BASE.into();
                            d.model = omarchyvr_config::ai::OPENROUTER_VOICE_MODEL.into();
                            d.action_model = omarchyvr_config::ai::OPENROUTER_ACTION_MODELS[0].into();
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
                d.command.clear();
            }
        }
        let kind = Kind::ALL[d.kind];
        egui::Grid::new("ai-editor").num_columns(2).spacing([14.0, 8.0]).show(ui, |ui| {
            ui.label(RichText::new("Name").color(p.dim));
            ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(220.0));
            ui.end_row();
            ui.label(RichText::new(if d.action_model.trim().is_empty() { "Model" } else { "Intent model" }).color(p.dim))
                .on_hover_text("Voice control: this model sorts a command into a kind (screens, apps, tabs, ...)");
            if kind == Kind::Anthropic || kind == Kind::ClaudeCli {
                let models: &[(&str, &str)] = if kind == Kind::ClaudeCli { &CLAUDE_CLI_MODELS } else { &CLAUDE_MODELS };
                let cur = models.iter().position(|(id, _)| *id == d.model).unwrap_or(0);
                ui.horizontal_wrapped(|ui| {
                    for (i, (_, n)) in models.iter().enumerate() {
                        if ui.selectable_label(i == cur, *n).clicked() {
                            d.model = models[i].0.to_string();
                        }
                    }
                });
                if !models.iter().any(|(id, _)| *id == d.model) {
                    d.model = models[0].0.to_string();
                }
            } else {
                ui.add(egui::TextEdit::singleline(&mut d.model).desired_width(260.0).hint_text(if kind == Kind::OpenAi { "the model name, as OpenAI lists it" } else { "e.g. qwen3:8b" }));
            }
            ui.end_row();
            if kind == Kind::ClaudeCli {
                ui.label(RichText::new("Command").color(p.dim)).on_hover_text("Where the Claude Code program is. Empty: claude, found on your PATH.");
                ui.add(egui::TextEdit::singleline(&mut d.command).desired_width(320.0).hint_text("optional, e.g. ~/.local/bin/claude"));
                ui.end_row();
            }
            if kind != Kind::Anthropic && kind != Kind::ClaudeCli {
                ui.label(RichText::new("Action model").color(p.dim))
                    .on_hover_text("Voice control: this model acts on the command with that kind's tools (empty: the model above does both)");
                ui.vertical(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut d.action_model).desired_width(260.0).hint_text("optional, e.g. openai/gpt-6-luna"));
                    if d.base_url.starts_with("https://openrouter.ai/") {
                        ui.horizontal_wrapped(|ui| {
                            for m in omarchyvr_config::ai::OPENROUTER_ACTION_MODELS {
                                if ui.selectable_label(d.action_model == m, m).clicked() {
                                    d.action_model = m.to_string();
                                }
                            }
                        });
                    }
                });
                ui.end_row();
            }
            if kind != Kind::ClaudeCli {
                ui.label(RichText::new("Base URL").color(p.dim));
                ui.add(egui::TextEdit::singleline(&mut d.base_url).desired_width(320.0).hint_text(kind.default_base().unwrap_or("http://127.0.0.1:11434/v1")));
                ui.end_row();
            }
            if kind == Kind::ClaudeCli {
                ui.label("");
                ui.label(RichText::new("No key: uses the Claude Code you are logged into.").size(12.0).color(p.dim));
                ui.end_row();
            } else if kind.needs_key() || !d.key.is_empty() {
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
            if kind != Kind::ClaudeCli {
                ui.label(RichText::new("Or from").color(p.dim));
                ui.add(egui::TextEdit::singleline(&mut d.key_env).desired_width(220.0).hint_text(kind.usual_env().unwrap_or("an environment variable")));
                ui.end_row();
            }
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
        pr.action_model = Some(d.action_model.trim().to_string()).filter(|m| !m.is_empty() && kind != Kind::Anthropic && kind != Kind::ClaudeCli);
        pr.command = Some(d.command.trim().to_string()).filter(|c| !c.is_empty() && kind == Kind::ClaudeCli);
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
    // Autofix and the default go through curl: the Claude CLI is for voice control only
    let curl: Vec<String> = app.core.ai.config.providers.iter().filter(|p| curl_capable(p)).map(|p| p.name.clone()).collect();
    // voice: every provider, and the built-in Claude CLI when the file has none
    let mut voice: Vec<String> = app.core.ai.config.providers.iter().map(|p| p.name.clone()).collect();
    if !voice.iter().any(|n| n == CLAUDE_CLI_NAME) {
        voice.push(CLAUDE_CLI_NAME.to_string());
    }
    plain(ui, |ui| {
        title(ui, "Which provider for what");
        if !curl.is_empty() {
            let refs: Vec<&str> = curl.iter().map(String::as_str).collect();
            let cur = app.core.ai.config.default.as_deref().and_then(|d| curl.iter().position(|n| n == d)).unwrap_or(0);
            if let Some(i) = widgets::selector(ui, icon::ROBOT, "Default", &refs, cur, true) {
                let n = curl[i].clone();
                app.core.ai_set_default(&n);
            }
        }
        for (f, ic) in [(Feature::Voice, icon::MICROPHONE), (Feature::Autofix, icon::WRENCH)] {
            let names = if f == Feature::Voice { &voice } else { &curl };
            if names.is_empty() {
                continue;
            }
            let mut opts: Vec<&str> = vec!["the default"];
            opts.extend(names.iter().map(String::as_str));
            let cur = app.core.ai.config.uses.get(&f).and_then(|u| names.iter().position(|n| n == u)).map_or(0, |i| i + 1);
            if let Some(i) = widgets::selector(ui, ic, f.label(), &opts, cur, true) {
                let n = (i > 0).then(|| names[i - 1].clone());
                app.core.ai_set_use(f, n.as_deref());
            }
            let line = match app.core.ai_resolve(f) {
                Ok(r) => (format!("{}: {} ({}), key {}", f.label(), r.provider, r.model, match r.key_source {
                    omarchyvr_config::ai::KeySource::File => "from ai.toml".to_string(),
                    omarchyvr_config::ai::KeySource::Env(e) => format!("from ${e}"),
                    omarchyvr_config::ai::KeySource::Session => "for this session only".to_string(),
                    omarchyvr_config::ai::KeySource::None => "not needed".to_string(),
                }), true),
                Err(e) => (format!("{}: {e}", f.label()), false),
            };
            let p = theme::pal(ui);
            ui.add(egui::Label::new(RichText::new(line.0).size(12.0).color(if line.1 { p.dim } else { p.warn })).wrap());
        }
        note(ui, "Autofix cannot use the Claude CLI (it sends its request over the network itself). Session-only keys work for Autofix (it runs in this app); voice control reads ai.toml and the environment only.");
    });
}

fn local(ui: &mut Ui, app: &mut PimaxApp) {
    let mut add = None;
    plain(ui, |ui| {
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
