//! Ready check > "Check VR games": one button, the result in plain words, the one fix per failure (docs/GAMES-CHECK.md).
use crate::app::PimaxApp;
use crate::theme::icon;
use crate::widgets::{self, chip, note, pal, spinner};
use eframe::egui::{RichText, Ui};
use pimax_gui_core::gamecheck::Status;

/// The card.
pub fn show(ui: &mut Ui, app: &mut PimaxApp) {
    if app.core.demo && app.core.gamecheck.checks.is_none() {
        app.core.game_check(); // demo data for the screenshots
    }
    let busy = app.core.game_check_busy();
    let (ok, warn, fail) = app.core.gamecheck.counts();
    let has = app.core.gamecheck.checks.is_some();
    widgets::section_with(
        ui,
        "VR games",
        Some("Why a Steam game shows nothing in the headset: checks the OpenXR and OpenVR runtime files, Monado, Steam's game container (a small test program runs inside it), Steam's VR setting and the game's last log. Start VR first for the full test."),
        |ui| {
            if has {
                chip(ui, &format!("{ok} ok"), pal(ui).ok);
                if warn > 0 {
                    chip(ui, &format!("{warn} to look at"), pal(ui).warn);
                }
                if fail > 0 {
                    chip(ui, &format!("{fail} failing"), pal(ui).fail);
                }
            }
        },
        |ui| {
        ui.horizontal_wrapped(|ui| {
            if widgets::ibutton(ui, icon::ARROWS_CLOCKWISE, "Check VR games", !busy).on_hover_text("pimax vr game-check: takes a few seconds").clicked() {
                app.core.game_check();
            }
            if busy {
                spinner(ui);
                ui.label(RichText::new("Testing inside Steam's game container...").color(pal(ui).muted));
            }
        });
        let Some(checks) = app.core.gamecheck.checks.clone() else { return };
        ui.add_space(6.0);
        for c in &checks {
            let col = match c.status {
                Status::Pass => pal(ui).ok,
                Status::Warn => pal(ui).warn,
                Status::Fail => pal(ui).fail,
                Status::Skip => pal(ui).muted,
            };
            ui.horizontal_wrapped(|ui| {
                chip(ui, c.status.word(), col);
                ui.add(eframe::egui::Label::new(RichText::new(&c.title)).wrap());
            });
            if !c.fix.is_empty() {
                note(ui, &c.fix);
            }
            if !c.copy.is_empty() {
                widgets::code_box(ui, std::slice::from_ref(&c.copy));
            }
            ui.add_space(2.0);
        }
        },
    );
}
