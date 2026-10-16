//! The "Screens" option of the desk (Work) settings: how many VR screens (1 to 4) the next Work start makes, and
//! the readiness line of the virtual outputs (docs/SCREENS.md).
use crate::app::PimaxApp;
use crate::theme;
use crate::widgets::note;
use eframe::egui::{self, RichText, Ui};
use pimax_gui_core::screens::{readiness, Level, MAX_SCREENS};

/// The selector, its note and the readiness line. `editable`: the desk settings can change now.
pub fn body(ui: &mut Ui, app: &mut PimaxApp, editable: bool, supported: bool) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Screens").color(theme::pal(ui).dim));
        let r = ui.add_enabled_ui(editable && supported, |ui| {
            for n in 1..=MAX_SCREENS {
                let on = app.core.toggles.screens == n;
                let label = n.to_string();
                let r = ui.selectable_label(on, &label);
                r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, on, format!("{n} screens")));
                if r.clicked() && !on {
                    app.core.toggles.screens = n;
                    app.core.save_state();
                }
            }
        });
        if !supported {
            r.response.on_disabled_hover_text("The installed pimax launcher has no --screens: install the new build");
        }
    });
    note(
        ui,
        match app.core.toggles.screens {
            1 => "One VR screen: the desktop.",
            _ => "The desktop in the centre, more screens left, right and above it. A screen shows once it has a window: \"Max, new window\" opens the next free one, \"Max, move this left\" moves an app there, \"Max, close the other windows\" shows one screen again.",
        },
    );
    let running = app.core.live.alive("wayvr");
    ui.horizontal_wrapped(|ui| {
        let can = running && !app.core.busy();
        let r = crate::widgets::ibutton(ui, crate::theme::icon::SQUARES_FOUR, "Put everything in its own window", can)
            .on_hover_text("Terminals together on one screen, the browser on another, the Pimax app on another, the rest on the next free one (the same as \"Max, put everything in its own window\")");
        if r.clicked() {
            app.core.screens_arrange(false);
        }
        let r = crate::widgets::ibutton(ui, crate::theme::icon::ARROWS_CLOCKWISE, "Put everything back together", can)
            .on_hover_text("Every app back on the main screen");
        if r.clicked() {
            app.core.screens_arrange(true);
        }
    });
    let (level, line) = readiness(&app.core.paths.session_dir, app.core.toggles.screens, running);
    let p = theme::pal(ui);
    let color = match level {
        Level::Ok => p.ok,
        Level::Warn => p.warn,
        Level::Info => p.dim,
    };
    ui.label(RichText::new(line).size(12.0).color(color));
}
