//! Desktop: the Work desk (WayVR). Passthrough, the preset, screen numbers and subtitles; in developer mode the
//! number of screens, the live WayVR controls, the settings of the next desk start and the voice tools.
use crate::app::PimaxApp;
use crate::pages::{editable, home, screens, tool_button, wayvr};
use crate::theme::{self, icon};
use crate::widgets::{self, section, seg_row, switch_row};
use eframe::egui::{self, Ui};
use omarchyvr_gui_core::app::Page;
use omarchyvr_gui_core::launcher::Preset;
use omarchyvr_gui_core::tools::ToolId;

/// Draw the page.
pub fn show(ui: &mut Ui, app: &mut PimaxApp) {
    let ed = editable(app);
    let caps = app.core.facts.caps.clone();
    let t = app.core.toggles;
    let dev = app.core.state.dev_mode;
    section(ui, "Desktop", None, |ui| {
        switch_row(ui, &mut app.core.toggles.passthrough, "Passthrough", Some("The room behind the screens (the headset's camera view)."), ed && (caps.no_passthrough || !t.passthrough));
        layout_picker(ui, app, ed, caps.preset);
        wayvr::screen_numbers(ui, app);
        home::subtitles_row(ui, app);
    });
    web_pages(ui, app);
    if !dev {
        return;
    }
    section(ui, "Screens", Some("How many VR screens the next Work start makes (docs/SCREENS.md)."), |ui| screens::body(ui, app, ed, caps.screens));
    wayvr::body(ui, app);
    section(ui, "Voice", None, |ui| {
        switch_row(ui, &mut app.core.toggles.voice, "Live captions", Some("A panel under the main screen with what you say (desk --voice); nothing is typed. The headset microphone sends zeros while the display is off (LESSONS row 19)."), ed && (caps.voice || t.voice));
        wayvr::captions_screen(ui, app);
        ui.horizontal_wrapped(|ui| {
            tool_button(ui, app, ToolId::VoiceTest, "Voice test");
            if widgets::ibutton(ui, icon::ROBOT, "AI settings", true).on_hover_text("The AI page: providers for voice commands and Autofix").clicked() {
                app.core.page = Page::Ai;
            }
            if widgets::ibutton(ui, icon::ARROW_SQUARE_OUT, "Voice commands (research)", true).clicked() {
                let f = app.core.paths.repo.join("analysis/research/omarchy-voice-commands.html");
                app.core.open_folder(&f);
            }
        });
    });
}

/// The "Layout" picker: six fixed, locked screen layouts, applied live while Work runs, plus Recenter screens.
fn layout_picker(ui: &mut Ui, app: &mut PimaxApp, ed: bool, supported: bool) {
    let running = app.core.live.alive("wayvr");
    let names: Vec<&str> = Preset::ALL.iter().map(|p| p.label()).collect();
    let cur = Preset::ALL.iter().position(|p| *p == app.core.toggles.preset).unwrap_or(0);
    let help = "Work 4: screens 1 main, 2 left, 3 right, 4 up (the default). Focus 3: 1, 2, 3 close together. Work 5: 1, 2, 3 plus 5 up-left and 6 up-right. Work 6: all six. Cinema: one big curved screen, the room dimmed. Huge cinema: one 11 m screen, the room black. Screens never move by themselves: no grabbing, no auto-reset. Recenter brings the whole layout in front of you.";
    // saved with the other settings; while Work runs it is also applied now, so it stays usable then
    let can = supported && !app.core.layout_busy() && (ed || running);
    if let Some(i) = seg_row(ui, "Layout", Some(help), &names, cur, can) {
        app.core.layout_choose(Preset::ALL[i], running);
    }
    ui.horizontal_wrapped(|ui| {
        let ok = running && !app.core.layout_busy();
        let r = widgets::ibutton(ui, icon::CROSSHAIR, "Recenter screens", ok).on_hover_text("The whole layout in front of you, same shape (the same as \"Max, recenter\"). Only while Work runs.");
        if r.clicked() {
            app.core.layout_recenter();
        }
        if app.core.layout_busy() {
            widgets::spinner(ui);
        }
    });
    let p = theme::pal(ui);
    let (text, color) = match &app.core.layout_result {
        Some((false, e)) => (e.clone(), p.warn_text),
        _ if !supported => ("The installed launcher has no layouts: install the current build.".to_string(), p.warn_text),
        _ => (omarchyvr_gui_core::screens::layout_status(app.core.toggles.preset, running), p.dim),
    };
    ui.label(egui::RichText::new(text).size(12.0).color(color));
}

/// "Web pages in VR": a page on a VR screen of the desk, and what the browser can do (docs/research/webxr.md).
fn web_pages(ui: &mut Ui, app: &mut PimaxApp) {
    use omarchyvr_gui_core::webvr::Verdict;
    if app.core.web.report.is_none() && !app.core.task_busy("webvr") {
        app.core.web_check();
    }
    section(
        ui,
        "Web pages in VR",
        Some("During Work the browser is a screen in the headset: open a page here and use it with the controller pointer. A page cannot take over the headset (WebXR): no Linux browser has OpenXR yet (docs/research/webxr.md)."),
        |ui| {
            let side = app.core.running_side();
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::TextEdit::singleline(&mut app.core.web.url).desired_width(300.0).hint_text("oceano-piratas.vercel.app"));
                let ok = side == Some(false) && !app.core.web.url.trim().is_empty();
                let r = widgets::ibutton(ui, icon::ARROW_SQUARE_OUT, "Open in VR", ok);
                let why = match side {
                    Some(false) => "Opens the page in your browser on the VR desk (a new tab when the browser is open).",
                    Some(true) => "Play is for games: switch to Work to see web pages in the headset.",
                    None => "Press Work first: web pages show on the VR screens of the desk.",
                };
                if r.on_hover_text(why).on_disabled_hover_text(why).clicked() {
                    app.core.web_open();
                }
            });
            if side != Some(false) {
                widgets::note(ui, if side == Some(true) { "Play runs: web pages are part of Work." } else { "Start Work to open a page in the headset." });
            }
            let pal = theme::pal(ui);
            match app.core.web.report.clone() {
                None => {
                    ui.horizontal(|ui| {
                        widgets::spinner(ui);
                        widgets::note(ui, "Checking the browser...");
                    });
                }
                Some(r) => {
                    let colour = |v: &Verdict| match v {
                        Verdict::Yes(_) => pal.ok,
                        Verdict::Maybe(_) => pal.warn_text,
                        Verdict::No(_) => pal.dim,
                    };
                    egui::Grid::new("webvr").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                        widgets::kv(ui, "Browser", r.browser.as_str());
                        let gpu = match &r.webgpu_used {
                            Some(u) => format!("{}; {u}", r.webgpu.text()),
                            None => r.webgpu.text().to_string(),
                        };
                        widgets::kv(ui, "WebGPU", egui::RichText::new(gpu).color(colour(&r.webgpu)));
                        widgets::kv(ui, "Page takes over the headset (WebXR)", egui::RichText::new(format!("not possible: {}", r.webxr.text())).color(colour(&r.webxr)));
                        widgets::kv(ui, "OpenXR for other apps", r.runtime.as_str());
                    });
                    if widgets::tbutton(ui, "Check again", !app.core.task_busy("webvr")).clicked() {
                        app.core.web_check();
                    }
                }
            }
        },
    );
}
