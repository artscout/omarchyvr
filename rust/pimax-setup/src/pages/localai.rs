//! Local AI (user decision 2026-10-16): the local voice models of Work. One switch (On / Off), a preset (Low /
//! Medium / High, each a router "Lara" + action model pair from the benchmark of docs/AI.md section 16), "Advanced:
//! choose models" (the per-slot pickers, Lara on/off, "Video memory for AI"), the state (download, start, test), and
//! Online (Jev) as the separate choice. The models load in Work sessions only (pimaxd's ai.server), never in Play.
use crate::app::PimaxApp;
use crate::theme::{self, icon};
use crate::widgets::{self, caption, card, chip, note, pal, title};
use eframe::egui::{self, RichText, Ui};
use pimax_config::ai::{asr_plan, Asr, LocalModel, Preset, RunsOn, VoiceLocal, ACTION_MODELS, PRESETS, ROUTER_MODELS};

pub fn show(ui: &mut Ui, app: &mut PimaxApp) {
    app.core.want_ai();
    app.core.want_localai();
    if !app.core.ai.loaded {
        card(ui, |ui| {
            ui.horizontal(|ui| {
                widgets::spinner(ui);
                ui.label("Reading ~/.config/pimax/ai.toml...");
            });
        });
        return;
    }
    let busy = app.core.busy();
    let on = app.core.ai.config.local_models_on() && app.core.ai_runs_on() != RunsOn::Online;
    let online = app.core.ai_online_provider();
    card(ui, |ui| {
        let p = theme::pal(ui);
        ui.horizontal(|ui| {
            title(ui, "Local AI");
            widgets::help(ui, "The models that understand voice commands the offline commands do not know (\"Max, ...\"). They run on this PC, in Work only (the desk for work and movies): Play, SteamVR and games never load them, and switching to Play unloads them. Nothing you say leaves this PC.");
            if app.core.demo {
                chip(ui, "demo: nothing is read, written or sent", pal(ui).info);
            }
        });
        let mut sw = on;
        if widgets::toggle(ui, &mut sw, "Local models", !busy).changed() {
            app.core.ai_set_local_models(sw, false);
        }
        if !on {
            note(ui, "Off: no model is loaded; only the offline commands work, unless Online is chosen below.");
        }
        if !app.core.ai.saved.is_empty() {
            ui.label(RichText::new(&app.core.ai.saved).size(11.5).color(p.faint));
        }
    });
    if on {
        ui.add_space(8.0);
        presets(ui, app, busy);
        ui.add_space(8.0);
        speech(ui, app, busy);
        ui.add_space(8.0);
        agent_mode(ui, app, busy);
        ui.add_space(8.0);
        advanced(ui, app, busy);
        ui.add_space(8.0);
        status(ui, app, busy);
    }
    ui.add_space(8.0);
    card(ui, |ui| {
        let p = theme::pal(ui);
        ui.horizontal(|ui| {
            title(ui, "Online (Jev) instead");
            widgets::help(ui, "A separate choice: Jev on OpenRouter sorts the command and an action model acts (about 3.4 s, 37/40 hard phrases). Your words and the names of your windows are sent. Exact phrases still run on this PC.");
        });
        match online {
            None => note(ui, "No online provider is set up (AI page, developer mode: Add OpenRouter)."),
            Some(_) => {
                let mut use_online = app.core.ai_runs_on() == RunsOn::Online;
                if widgets::toggle(ui, &mut use_online, "Use Online (Jev) for voice commands", !busy).changed() {
                    // Online on: the local models off; Online off: back to the local models
                    app.core.ai_set_local_models(!use_online, use_online);
                }
                if use_online {
                    ui.label(RichText::new("Online is on: the local models stay unloaded.").size(12.0).color(p.dim));
                }
            }
        }
    });
}

/// Low / Medium / High with what each costs and does.
fn presets(ui: &mut Ui, app: &mut PimaxApp, busy: bool) {
    let cur = app.core.ai.config.preset();
    card(ui, |ui| {
        let p = theme::pal(ui);
        title(ui, "How much of the PC the AI may use");
        for i in &PRESETS {
            ui.horizontal(|ui| {
                if ui.add_enabled(!busy, egui::RadioButton::new(cur == Some(i.preset), "")).clicked() && cur != Some(i.preset) {
                    app.core.ai_set_preset(i.preset);
                }
                ui.vertical(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(i.label).size(13.5).color(p.text));
                        if i.preset == Preset::Medium {
                            chip(ui, "recommended", pal(ui).ok);
                        }
                    });
                    ui.add(egui::Label::new(RichText::new(format!("{} \u{00b7} {}", i.size, i.bench)).size(11.5).color(p.dim)).wrap());
                });
            });
        }
        if cur.is_none() {
            ui.label(RichText::new("Models chosen by hand (Advanced).").size(12.0).color(p.dim));
        }
    });
}

/// Agent mode: goals of several steps ("Max, let's watch Dune, cinema mode") run step by step.
fn agent_mode(ui: &mut Ui, app: &mut PimaxApp, busy: bool) {
    card(ui, |ui| {
        title(ui, "Agent mode");
        let mut on = app.core.ai_voice_agent();
        if widgets::switch_row(ui, &mut on, "Agent mode", Some("A request with several things to do (\"Max, let's enter cinema mode, I want to watch Dune\", \"Max, set up work: terminals left, browser middle, YouTube up-right\") runs one step at a time: after each step the AI looks at the screens and picks the next one, up to 10 steps or 30 seconds. Each step shows in the headset. \"Max, cancel\" or \"Max, stop\" ends it after the step that is running. It never closes an app you did not ask to close. Off: such a request runs as one plan of up to 4 steps, decided at once."), !busy).changed() {
            app.core.ai_set_voice_agent(on);
        }
        // the optional big agent model (opt-in, default off; docs/AI.md section 18)
        let mut big = app.core.ai_agent_big();
        if widgets::switch_row(ui, &mut big, "Big agent model (Qwen3.6-27B)", Some("Needs ~17 GB of graphics memory; no gain measured yet. Starts only when the memory fits; never next to a game."), !busy && on).changed() {
            app.core.ai_set_agent_big(big);
        }
    });
}

/// "Speech recognition": Auto / Small (processor) / Large (graphics card), each with its measured numbers.
fn speech(ui: &mut Ui, app: &mut PimaxApp, busy: bool) {
    let choice = app.core.ai.config.asr;
    let free = app.core.localai.status.as_ref().and_then(|s| s.as_ref().ok()).and_then(|s| s.vram_free_mib).map(|f| f as u32);
    let st = app.core.localai.status.as_ref().and_then(|s| s.as_ref().ok()).cloned();
    card(ui, |ui| {
        let p = theme::pal(ui);
        ui.horizontal(|ui| {
            title(ui, "Speech recognition");
            widgets::help(ui, "Turns what you say into text. Small runs on the processor; Large is a bigger model on the graphics card that hears you better in noise and answers faster. If the large one is not available, the small one answers by itself, so voice never stops. The large model starts and stops with the other local models (Work only) and is English only. Voice control picks the model when it starts: restart voice, or the next Work start, after changing this.");
        });
        let names: Vec<&str> = Asr::ALL.iter().map(|a| a.label()).collect();
        let cur = Asr::ALL.iter().position(|a| *a == choice).unwrap_or(0);
        if let Some(i) = widgets::selector(ui, icon::SLIDERS, "Model", &names, cur, !busy) {
            app.core.ai_set_asr(Asr::ALL[i]);
        }
        for a in Asr::ALL {
            let line = format!("{}: {}", a.label(), a.numbers());
            ui.add(egui::Label::new(RichText::new(line).size(11.5).color(if a == choice { p.text } else { p.dim })).wrap());
        }
        // the decision in plain words, and what runs now
        // the server's own share counts as free
        let own = st.as_ref().and_then(|s| s.asr.as_ref()).and_then(|a| a.gpu_mib).unwrap_or(0) as u32;
        let plan = asr_plan(choice, true, free.map(|f| f + own));
        ui.add(egui::Label::new(RichText::new(&plan.why).size(12.0).color(p.dim)).wrap());
        ui.horizontal_wrapped(|ui| match st.as_ref().and_then(|s| s.asr.as_ref()) {
            Some(a) => chip(ui, &format!("large model: {}{}", if a.ready { "ready" } else { "loading" }, a.gpu_mib.map(|m| format!(", {:.1} GB of video memory", m as f64 / 1024.0)).unwrap_or_default()), if a.ready { pal(ui).ok } else { pal(ui).warn }),
            None => chip(ui, "large model: off (the small one answers)", pal(ui).muted),
        });
        if let (true, Some(m)) = (plan.gpu, st.as_ref().and_then(|s| s.asr_missing.clone())) {
            ui.add(egui::Label::new(RichText::new(format!("The large model cannot start: {m}. The small one answers.")).size(12.0).color(pal(ui).warn)).wrap());
        }
    });
}

/// "Advanced: choose models": Lara on / off, the router and action model pickers, Video memory for AI.
fn advanced(ui: &mut Ui, app: &mut PimaxApp, busy: bool) {
    let t = app.core.ai.config.two_stage();
    card(ui, |ui| {
        egui::CollapsingHeader::new(RichText::new("Advanced: choose models").size(14.0)).id_salt("local-ai-advanced").default_open(app.core.ai.config.preset().is_none()).show(ui, |ui| {
            let p = theme::pal(ui);
            let mut lara = app.core.ai.config.voice_local == VoiceLocal::TwoStage;
            if ui.add_enabled(!busy, egui::Checkbox::new(&mut lara, "Lara sorts the command first (two stages)")).changed() {
                app.core.ai_set_runs_on3(if lara { RunsOn::TwoStage } else { RunsOn::Simple });
            }
            note(ui, "Lara (the router, on the processor) sorts the command into one kind: screens, apps, tabs, typing, web, modes, room, recording, captions or system. At the same time the action model answers; when it is unsure and its answer is of another kind, it answers again with only the commands of Lara's kind. A command of several steps (\"new window and open the terminal\") runs as the model planned it, one step after the other.");
            if let Some(id) = model_picker(ui, "Router (Lara)", &ROUTER_MODELS, &t.router, false, !busy && lara) {
                app.core.ai_set_two_stage_model(true, id);
            }
            ui.add_space(4.0);
            if let Some(id) = model_picker(ui, "Action model", &ACTION_MODELS, &t.action, true, !busy) {
                app.core.ai_set_two_stage_model(false, id);
            }
            ui.add_space(4.0);
            const VRAM: [(Option<u32>, &str); 5] = [(None, "Auto (fits next to what runs)"), (Some(4096), "Up to 4 GB"), (Some(6144), "Up to 6 GB"), (Some(8192), "Up to 8 GB"), (Some(12288), "Up to 12 GB")];
            let vnames: Vec<&str> = VRAM.iter().map(|v| v.1).collect();
            let vcur = VRAM.iter().position(|v| v.0 == t.vram_mib).unwrap_or(0);
            if let Some(i) = widgets::selector(ui, icon::SLIDERS, "Video memory for AI", &vnames, vcur, !busy) {
                app.core.ai_set_vram(VRAM[i].0);
            }
            note(ui, "Auto measures the free video memory each time Work starts and keeps room for the session; when a game needs the memory the AI moves to a smaller model or to the processor by itself.");
            let _ = p;
        });
    });
}

/// A model picker: one row per model (name, recommended mark, download, memory, the audit's accuracy and speed).
fn model_picker(ui: &mut Ui, caption_text: &str, models: &[LocalModel], current: &str, vram: bool, enabled: bool) -> Option<&'static str> {
    let p = theme::pal(ui);
    caption(ui, caption_text);
    let mut pick = None;
    for m in models {
        ui.horizontal(|ui| {
            if ui.add_enabled(enabled, egui::RadioButton::new(m.id == current, "")).clicked() && m.id != current {
                pick = Some(m.id);
            }
            ui.vertical(|ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(m.label).size(13.5).color(p.text));
                    if m.recommended {
                        chip(ui, "recommended", pal(ui).ok);
                    }
                });
                let line = format!("{:.1} GB download, {:.1} GB {} \u{00b7} {} \u{00b7} licence {}", m.download_mib as f64 / 1024.0, m.memory_mib as f64 / 1024.0, if vram { "of video memory" } else { "of RAM, no video memory" }, m.bench, m.licence);
                ui.add(egui::Label::new(RichText::new(line).size(11.5).color(p.dim)).wrap());
            });
        });
    }
    pick
}

/// The state: download (with sizes and licences), Start / Test / Stop, Lara and the model, the video memory.
fn status(ui: &mut Ui, app: &mut PimaxApp, busy: bool) {
    let t = app.core.ai.config.two_stage();
    let status = app.core.localai.status.clone();
    card(ui, |ui| {
        let p = theme::pal(ui);
        ui.horizontal(|ui| {
            title(ui, "State");
            widgets::help(ui, "Work starts the models by itself and Play stops them. If Lara stops, commands go to the model alone; if the model stops too, only the offline commands work.");
        });
        let Some(Ok(s)) = status else {
            if let Some(Err(e)) = &app.core.localai.status {
                ui.label(RichText::new(e).size(12.0).color(p.warn));
            }
            return;
        };
        let missing: Vec<&str> = [t.router.as_str(), t.action.as_str()].into_iter().filter(|id| !s.has(id)).collect();
        if !missing.is_empty() {
            let mib: u32 = missing.iter().filter_map(|id| pimax_config::ai::local_model(id)).map(|m| m.download_mib).sum();
            ui.label(RichText::new(format!("Not downloaded yet: {}.", missing.join(", "))).size(12.5).color(p.warn));
            let tune = missing.contains(&"lara");
            let label = if tune { format!("Download ({:.1} GB) and tune Lara", mib as f64 / 1024.0) } else { format!("Download ({:.1} GB)", mib as f64 / 1024.0) };
            if tune {
                note(ui, "Tuning teaches Lara this desk's commands: about an hour once, in the background on the processor only, after the download (VR keeps running; Laya as published sorts the commands meanwhile).");
            }
            if widgets::primary(ui, &label, !busy).on_hover_text("pimax-ai install --two-stage: into ~/.local/share/pimax/ai, checked and resumable").clicked() {
                app.core.localai_install_two_stage();
            }
            for id in &missing {
                if let Some(m) = pimax_config::ai::local_model(id) {
                    ui.label(RichText::new(format!("{}: licence {}", m.label, m.licence)).size(11.5).color(p.faint));
                }
            }
            return;
        }
        if s.lara_tuning {
            note(ui, "Lara is being tuned to your commands in the background (about an hour, processor only). Laya as published sorts the commands meanwhile.");
        }
        ui.horizontal_wrapped(|ui| {
            match &s.router {
                Some(r) => chip(ui, &format!("Lara: {}", if r.ready { "ready" } else { "loading" }), if r.ready { pal(ui).ok } else { pal(ui).warn }),
                None => chip(ui, "Lara: off", pal(ui).muted),
            }
            match &s.running {
                Some(r) => chip(ui, &format!("model: {} ({})", r.model.trim_end_matches(".gguf"), r.mode), if s.ready { pal(ui).ok } else { pal(ui).warn }),
                None => chip(ui, "model: off", pal(ui).muted),
            }
        });
        if let (Some(free), Some(total)) = (s.vram_free_mib, s.vram_total_mib) {
            let used_ai = s.gpu_mib.unwrap_or(0);
            ui.label(RichText::new(format!("Video memory: the AI uses {:.1} GB, {:.1} GB of {:.0} GB free{}", used_ai as f64 / 1024.0, free as f64 / 1024.0, total as f64 / 1024.0, s.session.as_deref().map(|x| format!(" (next to {x})")).unwrap_or_default())).size(12.0).color(p.dim));
        }
        ui.horizontal_wrapped(|ui| {
            if s.running.is_none() || s.router.is_none() {
                if widgets::ibutton(ui, icon::PLAY, "Start", !busy).on_hover_text("pimax-ai up: Lara on the processor, the action model fitted to the free video memory").clicked() {
                    app.core.localai_up();
                }
            } else {
                if widgets::ibutton(ui, icon::LIGHTNING, "Test", !busy).on_hover_text("Checks in order: Lara, the action model, speech recognition, and the agent model when it is on").clicked() {
                    app.core.localai_test();
                }
                if widgets::ibutton(ui, icon::STOP, "Stop", !busy).on_hover_text("pimax-ai down").clicked() {
                    app.core.localai_stop();
                }
            }
        });
        test_result(ui, app);
    });
}

/// The result of the one Test: a compact line per check (mark, name, time), the details behind a "?".
fn test_result(ui: &mut Ui, app: &PimaxApp) {
    let p = theme::pal(ui);
    if app.core.localai.testing {
        ui.horizontal(|ui| {
            widgets::spinner(ui);
            ui.label(RichText::new("Testing...").size(12.0).color(p.dim));
        });
        return;
    }
    match &app.core.localai.test {
        Some(Ok(checks)) => {
            for c in checks {
                ui.horizontal(|ui| {
                    let (mark, color) = match c.state.as_str() {
                        "ok" => (icon::CHECK_CIRCLE, p.ok),
                        "fail" => (icon::X_CIRCLE, p.warn),
                        _ => (icon::CIRCLE, p.faint),
                    };
                    let time = if c.state == "skip" { "skipped".to_string() } else { format!("{} ms", c.ms) };
                    ui.label(RichText::new(format!("{mark} {}  {time}", c.label())).size(12.0).color(color));
                    widgets::help(ui, &c.detail);
                });
            }
        }
        Some(Err(e)) => {
            ui.label(RichText::new(e).size(12.0).color(p.warn));
        }
        None => {}
    }
}
