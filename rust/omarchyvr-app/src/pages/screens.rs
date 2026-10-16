//! The "Screens" option of the desk (Work) settings: how many VR screens (1 to 6) the next Work start makes, and
//! the readiness line of the virtual outputs (docs/SCREENS.md).
use crate::app::PimaxApp;
use crate::theme;
use crate::widgets;
use eframe::egui::{RichText, Ui};
use omarchyvr_gui_core::screens::{readiness_from, Level, MAX_SCREENS};

/// The choice, the arrange buttons and the readiness line. `editable`: the desk settings can change now.
pub fn body(ui: &mut Ui, app: &mut PimaxApp, editable: bool, supported: bool) {
    let names: Vec<String> = (1..=MAX_SCREENS).map(|n| n.to_string()).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let help = if supported {
        "The desktop in the centre, more screens left, right and above it (5 and 6 add the two above left and right). A screen shows once it has a window: \"Max, new window\" opens the next free one, \"Max, move this left\" moves an app there, \"Max, close the other windows\" shows one screen again."
    } else {
        "The installed pimax launcher has no --screens: install the new build."
    };
    let cur = usize::from(app.core.toggles.screens).saturating_sub(1);
    if let Some(i) = widgets::seg_row(ui, "Screens", Some(help), &refs, cur, editable && supported) {
        app.core.toggles.screens = u8::try_from(i + 1).unwrap_or(1);
    }
    let running = app.core.live.alive("wayvr");
    ui.horizontal_wrapped(|ui| {
        let can = running && !app.core.busy();
        let r = widgets::ibutton(ui, theme::icon::SQUARES_FOUR, "Put everything in its own window", can)
            .on_hover_text("Terminals together on one screen, the browser on another, the Pimax app on another, the rest on the next free one (the same as \"Max, put everything in its own window\")");
        if r.clicked() {
            app.core.screens_arrange(false);
        }
        let r = widgets::ibutton(ui, theme::icon::ARROWS_CLOCKWISE, "Put everything back together", can).on_hover_text("Every app back on the main screen");
        if r.clicked() {
            app.core.screens_arrange(true);
        }
    });
    let (level, line) = readiness_from(app.core.facts.screens_report.clone(), app.core.toggles.screens, running);
    let p = theme::pal(ui);
    let color = match level {
        Level::Ok => p.ok,
        Level::Warn => p.warn,
        Level::Info => p.dim,
    };
    ui.label(RichText::new(line).size(12.0).color(color));
}
