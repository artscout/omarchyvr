//! The pages. Each `show` draws one page of [`crate::app::PimaxApp`].
pub mod ai;
pub mod autofix;
pub mod backgrounds;
pub mod checks;
pub mod desk;
pub mod desktop;
pub mod developer;
pub mod display;
pub mod gamecheck;
pub mod home;
pub mod localai;
pub mod logs;
pub mod onboarding;
pub mod profiles;
pub mod record;
pub mod room;
pub mod screens;
pub mod setup;
pub mod system;
pub mod tools;
pub mod tracking;
pub mod wayvr;

use crate::app::PimaxApp;
use eframe::egui::{self, Ui};
use pimax_gui_core::app::DeskPhase;
use pimax_gui_core::tools::ToolId;

/// Can the switches of the next start change (nothing runs, no job)?
pub fn editable(app: &PimaxApp) -> bool {
    matches!(app.core.phase(), DeskPhase::Idle | DeskPhase::Foreign) && !app.core.busy()
}

/// A tool as a button (disabled with its reasons when it cannot start).
pub fn tool_button(ui: &mut Ui, app: &mut PimaxApp, t: ToolId, label: &str) {
    let a = app.core.tool_availability(t);
    let can = a.ready() && !app.core.busy();
    let b = crate::widgets::tbutton(ui, label, can);
    let b = if can { b } else { b.on_disabled_hover_text(a.blockers.iter().map(|x| x.text.clone()).collect::<Vec<_>>().join(" ")) };
    if b.clicked() {
        app.core.run_tool(t);
    }
}

/// Two columns of cards side by side (`add(0, ui)` on the left, `add(1, ui)` on the right), or one under the other
/// when the page is narrower than `min_w` per column.
pub fn two_columns(ui: &mut Ui, min_w: f32, mut add: impl FnMut(usize, &mut Ui)) {
    let w = ui.available_width();
    if w < 2.0 * min_w + 12.0 {
        add(0, ui);
        add(1, ui);
        return;
    }
    let half = ((w - 12.0) / 2.0).floor();
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        for i in 0..2 {
            ui.allocate_ui_with_layout(egui::vec2(half, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_width(half);
                ui.spacing_mut().item_spacing.y = 12.0;
                add(i, ui);
            });
        }
    });
}
