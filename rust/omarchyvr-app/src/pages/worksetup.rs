//! The Work first run (docs/WORK-FIRST-RUN.md): two questions the first time Work is pressed, at the desktop, before
//! VR starts: the voice assistant (Lara and the "brain") and speech to text. The answers go into the shared settings
//! (ai.toml, the Work switches); then Work starts by itself. The AI page opens it again to change the answers. Play
//! never shows it.
use crate::app::PimaxApp;
use crate::theme::{self, icon};
use crate::widgets::{self, chip, inset as card, note, pal, title};
use eframe::egui::{self, RichText, Ui};
use omarchyvr_config::ai::CLAUDE_MODELS;
use omarchyvr_config::claude_cli::ClaudeCliStatus;
use omarchyvr_gui_core::worksetup::{self as ws, Brain, ItemState, Speech, Step};

/// Draw the page.
pub fn show(ui: &mut Ui, app: &mut PimaxApp) {
    let p = theme::pal(ui);
    widgets::card(ui, |ui| {
        widgets::title_row(ui, "Set up Work", None, |ui| {
            if widgets::ghost(ui, icon::X_CIRCLE, "Close", true).on_hover_text("Close without saving (Work does not start; the questions come again next time)").clicked() {
                app.core.ws_close();
            }
        });
        let intro = if app.core.work_setup.start_after {
            "Work is your desk in VR: your screens, voice commands and an AI helper. Two questions, then Work starts. You can change the answers later on the AI page."
        } else {
            "The Work questions: the voice assistant and speech to text. Saved for the next Work."
        };
        ui.add(egui::Label::new(RichText::new(intro).size(13.0).color(p.dim)).wrap());
        ui.add_space(6.0);
        stepper(ui, app);
    });
    ui.add_space(10.0);
    match app.core.work_setup.step {
        Step::Assistant => assistant(ui, app),
        Step::Speech => speech(ui, app),
        Step::Get => get(ui, app),
    }
}

/// 1 Voice assistant - 2 Speech to text (- 3 Download).
fn stepper(ui: &mut Ui, app: &PimaxApp) {
    let p = theme::pal(ui);
    let step = app.core.work_setup.step;
    let get = step == Step::Get || !app.core.work_setup.needed().is_empty();
    ui.horizontal_wrapped(|ui| {
        let mut steps = vec![(Step::Assistant, "1  Voice assistant"), (Step::Speech, "2  Speech to text")];
        if get {
            steps.push((Step::Get, "3  Download"));
        }
        for (i, (s, l)) in steps.iter().enumerate() {
            if i > 0 {
                ui.label(RichText::new("\u{2014}").size(12.0).color(p.faint));
            }
            chip(ui, l, if *s == step { p.accent } else { p.faint });
        }
    });
}

/// One choice row: a radio, its name, one line of what it means.
fn choice(ui: &mut Ui, on: bool, enabled: bool, name: &str, what: &str) -> bool {
    let p = theme::pal(ui);
    let mut clicked = false;
    ui.horizontal(|ui| {
        if ui.add_enabled(enabled, egui::RadioButton::new(on, "")).clicked() {
            clicked = true;
        }
        ui.vertical(|ui| {
            let r = ui.add(egui::Label::new(RichText::new(name).size(13.5).color(if enabled { p.text } else { p.faint })).sense(egui::Sense::click()));
            if enabled && r.clicked() {
                clicked = true;
            }
            ui.add(egui::Label::new(RichText::new(what).size(11.5).color(p.dim)).wrap());
        });
    });
    clicked
}

/// A masked key field with Show.
fn key_field(ui: &mut Ui, app: &mut PimaxApp, hint: &str) {
    let w = &mut app.core.work_setup;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Key").size(12.5));
        ui.add(egui::TextEdit::singleline(&mut w.key).password(!w.show_key).desired_width(300.0).hint_text(hint));
        if ui.small_button(if w.show_key { "Hide" } else { "Show" }).clicked() {
            w.show_key = !w.show_key;
        }
    });
}

/// The result of a connection test (a typed key / API, or the Claude CLI).
fn test_row(ui: &mut Ui, app: &mut PimaxApp) {
    let cli = app.core.work_setup.choices.brain == Brain::ClaudeCli;
    let testing = if cli { app.core.ai.claude_testing } else { app.core.work_setup.testing };
    let res = if cli { app.core.ai.claude_test.clone() } else { app.core.work_setup.test.clone() };
    ui.horizontal_wrapped(|ui| {
        if widgets::ghost(ui, icon::PULSE, if testing { "Testing..." } else { "Test" }, !testing).on_hover_text("Sends one fixed line (nothing from this PC) and shows the answer time").clicked() {
            app.core.ws_test();
        }
        match &res {
            Some(Ok(l)) => {
                ui.label(RichText::new(l).size(12.0).color(pal(ui).ok));
            }
            Some(Err(e)) => {
                ui.add(egui::Label::new(RichText::new(e).size(12.0).color(pal(ui).warn)).wrap());
            }
            None => {}
        }
    });
}

/// Page 1: who answers, and Lara.
fn assistant(ui: &mut Ui, app: &mut PimaxApp) {
    let on_disk = app.core.work_setup.on_disk.clone();
    let brain = app.core.work_setup.choices.brain;
    card(ui, |ui| {
        title(ui, "Who answers your voice requests");
        note(ui, "Exact phrases (\"Max, four screens\", \"Max, volume up\") always run on this PC without any AI. This is who understands everything else.");
        ui.add_space(4.0);
        // Claude through the Claude Code program on this PC
        if choice(ui, brain == Brain::ClaudeCli, true, "Claude: the Claude Code program on this PC", "Uses your installed Claude Code with its own sign-in (your Claude account; it counts on your plan). This app never reads or copies that login. Needs the internet.") {
            app.core.work_setup.choices.brain = Brain::ClaudeCli;
        }
        if brain == Brain::ClaudeCli {
            widgets::inset(ui, |ui| {
                let p = theme::pal(ui);
                match on_disk.as_ref().and_then(|d| d.claude.clone()) {
                    Some(ClaudeCliStatus::Found { version, .. }) => chip(ui, &format!("Claude Code {} found", version.trim_end_matches("(Claude Code)").trim()), p.ok),
                    Some(ClaudeCliStatus::NotFound) => {
                        ui.add(egui::Label::new(RichText::new("Claude Code is not installed on this PC. Install it, sign in once, then press Test.").size(12.0).color(p.warn)).wrap());
                    }
                    Some(ClaudeCliStatus::Broken { error, .. }) => {
                        ui.add(egui::Label::new(RichText::new(format!("Claude Code does not answer: {error}")).size(12.0).color(p.warn)).wrap());
                    }
                    None => chip(ui, "looking for Claude Code...", p.faint),
                }
                ui.horizontal_wrapped(|ui| {
                    test_row(ui, app);
                    if app.core.ai.claude_test.as_ref().is_some_and(|r| r.as_ref().is_err_and(|e| e.contains("logged in"))) && widgets::ghost(ui, icon::LINK_SIMPLE, "Sign in to Claude", true).clicked() {
                        app.core.claude_login();
                    }
                });
            });
        }
        // Claude with the user's own API key
        if choice(ui, brain == Brain::ClaudeKey, true, "Claude with my Anthropic API key", "Your own key, billed to your Anthropic account. Saved only in ~/.config/omarchyvr/ai.toml, readable only by you (never in a log).") {
            app.core.work_setup.choices.brain = Brain::ClaudeKey;
        }
        if brain == Brain::ClaudeKey {
            widgets::inset(ui, |ui| {
                let saved = app.core.ws_saved_key();
                key_field(ui, app, if saved { "a key is saved: leave empty to keep it" } else { "sk-ant-..." });
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Model").size(12.5));
                    for (id, label) in CLAUDE_MODELS {
                        let on = app.core.work_setup.choices.claude_model == id;
                        if ui.radio(on, label).clicked() {
                            app.core.work_setup.choices.claude_model = id.to_string();
                        }
                    }
                });
                test_row(ui, app);
            });
        }
        // a model on this PC
        if choice(ui, brain == Brain::Local, true, "A model on this PC (Qwen3.5-4B)", "2.6 GB download (Apache-2.0). Nothing you say leaves this PC. Runs on the processor: no video memory, about 1.3 s per request.") {
            app.core.work_setup.choices.brain = Brain::Local;
        }
        if brain == Brain::Local {
            widgets::inset(ui, |ui| {
                let mut gpu = app.core.work_setup.choices.local_gpu;
                if widgets::switch_row(ui, &mut gpu, "Use the graphics card", Some("Faster (about 0.35 s per request) but about 3 GB of video memory while Work runs, and it can cost a VR frame per request. Never during Play: Play always frees the graphics card."), true).changed() {
                    app.core.work_setup.choices.local_gpu = gpu;
                }
                if on_disk.as_ref().is_some_and(|d| d.qwen4b && d.runtime) {
                    chip(ui, "already on this PC", pal(ui).ok);
                }
            });
        }
        // any other compatible API
        if choice(ui, brain == Brain::Compatible, true, "Another compatible API", "Any service with an OpenAI-compatible API (OpenRouter, a hosted model, a server on your network): its base URL, your key and a model.") {
            app.core.work_setup.choices.brain = Brain::Compatible;
        }
        if brain == Brain::Compatible {
            widgets::inset(ui, |ui| {
                let c = &mut app.core.work_setup.choices;
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Base URL").size(12.5));
                    ui.add(egui::TextEdit::singleline(&mut c.base_url).desired_width(320.0).hint_text("https://openrouter.ai/api/v1"));
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Model").size(12.5));
                    ui.add(egui::TextEdit::singleline(&mut c.model).desired_width(260.0).hint_text("the model name the API lists"));
                });
                let saved = app.core.ws_saved_key();
                key_field(ui, app, if saved { "a key is saved: leave empty to keep it" } else { "the key or token (some servers need none)" });
                test_row(ui, app);
            });
        }
        if choice(ui, brain == Brain::None, true, "None", "Only the built-in voice commands. Nothing leaves this PC.") {
            app.core.work_setup.choices.brain = Brain::None;
        }
    });
    ui.add_space(10.0);
    lara(ui, app);
    ui.add_space(10.0);
    nav(ui, app);
}

/// Lara: on / off (only with a brain that acts after it).
fn lara(ui: &mut Ui, app: &mut PimaxApp) {
    let c = app.core.work_setup.choices.clone();
    let d = app.core.work_setup.on_disk.clone();
    card(ui, |ui| {
        let p = theme::pal(ui);
        title(ui, "Lara: sorts your commands on this PC");
        let usable = c.brain.takes_lara();
        let mut on = c.lara && usable;
        let r = widgets::switch_row(ui, &mut on, "Install Lara", Some("Lara is a small model on the processor (no video memory). Before the brain acts, it picks what kind of command it is, so the brain sees only the right tools: more accurate, fewer tokens."), usable);
        if r.changed() {
            app.core.work_setup.choices.lara = on;
        }
        ui.add(egui::Label::new(RichText::new("2.7 GB download (Apache-2.0). After the download it tunes itself to your voice commands in the background: about an hour on the processor, at low priority; it works meanwhile.").size(11.5).color(p.dim)).wrap());
        match c.brain {
            Brain::ClaudeCli => note(ui, "Not used with Claude Code: it reads every command itself. Choose \"Claude with my Anthropic API key\" to use Lara with Claude."),
            Brain::None => note(ui, "Lara needs a brain that acts after it: choose one above."),
            _ => {}
        }
        if let Some(d) = d.as_ref().filter(|d| d.laya) {
            chip(ui, if d.lara { "already on this PC, tuned to your commands" } else { "already on this PC" }, p.ok);
        }
    });
}

/// Page 2: speech to text, and what happens next.
fn speech(ui: &mut Ui, app: &mut PimaxApp) {
    let d = app.core.work_setup.on_disk.clone().unwrap_or_default();
    let cur = app.core.work_setup.choices.speech;
    card(ui, |ui| {
        let p = theme::pal(ui);
        title(ui, "Speech to text");
        note(ui, "Turns what you say in the headset into text for the voice commands and the subtitles. Always on this PC.");
        ui.add_space(4.0);
        if choice(ui, cur == Speech::Cpu, true, "On this PC, the processor (recommended)", "Parakeet: no video memory, English and Spanish, about 0.14 s per sentence.") {
            app.core.work_setup.choices.speech = Speech::Cpu;
        }
        if !d.parakeet && app.core.work_setup.on_disk.is_some() {
            ui.add(egui::Label::new(RichText::new("The speech model is missing on this PC (the installer puts it in ~/models): run the installer again.").size(12.0).color(p.warn)).wrap());
        }
        let next_to_model = ws::gpu_speech_possible(&app.core.work_setup.choices);
        if choice(ui, cur == Speech::Gpu, d.whisper && next_to_model, "On the graphics card", "Whisper large-v3-turbo: a little better in noise, about 0.9 GB of video memory while Work runs (never during Play); it can make VR stutter.") {
            app.core.work_setup.choices.speech = Speech::Gpu;
        }
        if !d.whisper && app.core.work_setup.on_disk.is_some() {
            ui.label(RichText::new("Not installed on this PC.").size(11.5).color(p.faint));
        } else if !next_to_model {
            ui.label(RichText::new("Only with the model on this PC (it runs next to it).").size(11.5).color(p.faint));
        }
        if choice(ui, cur == Speech::Off, true, "Off", "No voice commands and no subtitles in Work.") {
            app.core.work_setup.choices.speech = Speech::Off;
        }
    });
    ui.add_space(10.0);
    // what Finish will do
    let need = app.core.work_setup.needed();
    card(ui, |ui| {
        let p = theme::pal(ui);
        title(ui, "Next");
        if app.core.work_setup.on_disk.is_none() {
            ui.label(RichText::new("Looking at what is on this PC...").size(12.5).color(p.dim));
        } else if need.is_empty() {
            ui.label(RichText::new("Nothing to download: everything is on this PC.").size(12.5).color(p.ok));
        } else {
            for id in &need {
                if let Some((_, bytes, label)) = ws::ITEMS.iter().find(|x| x.0 == *id) {
                    ui.label(RichText::new(format!("{label}: {}", ws::gb(*bytes))).size(12.5).color(p.text));
                }
            }
            ui.add(egui::Label::new(RichText::new("Files already on this PC are checked (SHA-256) and kept; a stopped download goes on where it stopped.").size(11.5).color(p.dim)).wrap());
        }
    });
    ui.add_space(10.0);
    nav(ui, app);
}

/// Page 3: the downloads with progress.
fn get(ui: &mut Ui, app: &mut PimaxApp) {
    let Some(st) = app.core.work_setup.get.clone() else {
        nav(ui, app);
        return;
    };
    let (items, frac, note_line, running) = match st.lock() {
        Ok(s) => (s.items.clone(), s.fraction(), s.note.clone(), s.finished.is_none()),
        Err(_) => (Vec::new(), 0.0, String::new(), false),
    };
    card(ui, |ui| {
        let p = theme::pal(ui);
        title(ui, if running { "Downloading" } else { "Downloads" });
        for it in &items {
            ui.label(RichText::new(&it.label).size(13.0).color(p.text));
            let line = |ui: &mut Ui, text: String, c: egui::Color32| {
                ui.add(egui::Label::new(RichText::new(text).size(12.0).color(c)).wrap());
            };
            match &it.state {
                ItemState::Running => {
                    let k = if it.total > 0 { (it.done as f32 / it.total as f32).min(1.0) } else { 0.0 };
                    ui.add(egui::ProgressBar::new(k).desired_width(ui.available_width()).text(format!("{} of {}", ws::gb(it.done.min(it.total)), ws::gb(it.total))));
                }
                ItemState::Here => line(ui, "already on this PC (checked)".into(), p.ok),
                ItemState::Done => line(ui, "done (SHA-256 checked)".into(), p.ok),
                ItemState::Waiting => line(ui, format!("waiting ({})", ws::gb(it.total)), p.dim),
                ItemState::Failed(e) => line(ui, format!("stopped: {}", ws::plain_download_error(e)), p.warn),
                ItemState::Cancelled => line(ui, "cancelled (the part already here is kept)".into(), p.dim),
            }
        }
        ui.add_space(4.0);
        ui.add(egui::ProgressBar::new(frac).desired_width(ui.available_width()).text(format!("{:.0} % in all", frac * 100.0)));
        if !note_line.is_empty() {
            ui.add(egui::Label::new(RichText::new(&note_line).size(11.5).color(p.dim)).truncate());
        }
        if running {
            ui.add(egui::Label::new(RichText::new("You can keep using the PC meanwhile. Work starts when this is done.").size(12.0).color(p.dim)).wrap());
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        }
    });
    ui.add_space(10.0);
    nav(ui, app);
}

/// Back / Next / Finish / Cancel, and the plain-words problem.
fn nav(ui: &mut Ui, app: &mut PimaxApp) {
    let p = theme::pal(ui);
    if let Some(e) = app.core.work_setup.error.clone() {
        widgets::status(ui, &e, p.warn);
    }
    let step = app.core.work_setup.step;
    let getting = app.core.work_setup.getting();
    let need = app.core.work_setup.needed();
    let start = app.core.work_setup.start_after;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        match step {
            Step::Assistant => {
                if widgets::action(ui, icon::CARET_RIGHT, "Next", widgets::Kind::Primary, true, 200.0).clicked() {
                    app.core.ws_next();
                }
            }
            Step::Speech => {
                if widgets::action(ui, "", "Back", widgets::Kind::Secondary, true, 140.0).clicked() {
                    app.core.ws_back();
                }
                let label = match (need.is_empty(), start) {
                    (true, true) => "Start Work".to_string(),
                    (true, false) => "Save".to_string(),
                    (false, true) => format!("Download {} and start Work", ws::gb(ws::bytes_of(&need))),
                    (false, false) => format!("Download {} and save", ws::gb(ws::bytes_of(&need))),
                };
                let ok = app.core.work_setup.on_disk.is_some();
                if widgets::action(ui, if start { icon::DESKTOP } else { icon::CHECK_CIRCLE }, &label, widgets::Kind::Primary, ok, 320.0).clicked() {
                    app.core.ws_finish();
                }
            }
            Step::Get if getting => {
                if widgets::action(ui, icon::STOP, "Cancel", widgets::Kind::Danger, true, 160.0).on_hover_text("Stops the download; what is already here is kept for the next try").clicked() {
                    app.core.ws_cancel();
                }
            }
            Step::Get => {
                if widgets::action(ui, "", "Back", widgets::Kind::Secondary, true, 140.0).clicked() {
                    app.core.ws_back();
                }
                if widgets::action(ui, icon::ARROWS_CLOCKWISE, "Try again", widgets::Kind::Primary, true, 200.0).on_hover_text("Goes on where it stopped").clicked() {
                    app.core.ws_finish();
                }
            }
        }
    });
}
