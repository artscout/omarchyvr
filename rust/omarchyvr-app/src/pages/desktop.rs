//! Desktop: the Work desk (WayVR). Passthrough, the preset, screen numbers and subtitles; in developer mode the
//! number of screens, the live WayVR controls, the settings of the next desk start and the voice tools.
use crate::app::PimaxApp;
use crate::pages::{editable, home, screens, tool_button, wayvr};
use crate::theme::icon;
use crate::widgets::{self, section, seg_row, switch_row};
use eframe::egui::Ui;
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
        const PRESETS: [(Preset, &str); 4] = [(Preset::None, "None"), (Preset::Cinema, "Cinema"), (Preset::Work, "Work"), (Preset::Assistant, "Assistant")];
        let names: Vec<&str> = PRESETS.iter().map(|x| x.1).collect();
        let cur = PRESETS.iter().position(|x| x.0 == app.core.toggles.preset).unwrap_or(0);
        if let Some(i) = seg_row(
            ui,
            "Preset",
            Some("None: the screens as you placed them. Cinema: curved screens, passthrough dimmed. Work: flat, sharper screens. Assistant: voice captions on. Applies at the next desk start."),
            &names,
            cur,
            ed && caps.preset,
        ) {
            app.core.toggles.preset = PRESETS[i].0;
        }
        wayvr::screen_numbers(ui, app);
        home::subtitles_row(ui, app);
    });
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
