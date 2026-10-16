//! Ready check > "Check VR games": one button, the result in plain words, the one fix per failure (docs/GAMES-CHECK.md).
use crate::app::PimaxApp;
use crate::theme::icon;
use crate::widgets::{self, chip, note, pal, spinner};
use eframe::egui::{RichText, Ui};
use omarchyvr_gui_core::gamecheck::Status;

/// "Games that run with SteamVR": the per-game runtime list (crashes with xrizer), with a way to change each one.
fn runtimes(ui: &mut Ui, app: &mut PimaxApp) {
    use omarchyvr_gui_core::gameruntime::Games;
    let list = app.core.game_runtime_list();
    let next = app.core.facts.next_game.clone();
    let busy = app.core.busy();
    widgets::section(
        ui,
        "Games that run with SteamVR",
        Some("Some games crash with xrizer (the OpenVR layer for Monado), for example Half-Life: Alyx. They are listed here, by a built-in list and by the app when it sees one crash. Pick one under 'Next Play is for' and Play starts the session with SteamVR for it. Other games are not affected; xrizer comes back at the next normal Play."),
        |ui| {
            if let Some(n) = app.core.game_watch.crash_note.clone() {
                note(ui, &n);
            }
            for e in &list {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&e.name).strong());
                    if e.runtime == Games::SteamVr {
                        chip(ui, "Runs with SteamVR (crashes with xrizer)", pal(ui).warn);
                    } else {
                        chip(ui, "Runs with xrizer", pal(ui).ok);
                    }
                    let (label, to) = if e.runtime == Games::SteamVr { ("Use xrizer", "xrizer") } else { ("Use SteamVR", "steamvr") };
                    if ui.add_enabled(!busy, eframe::egui::Button::new(label)).clicked() {
                        app.core.set_game_runtime(&e.appid, to, &e.name);
                    }
                    let is_next = next.as_deref() == Some(e.appid.as_str());
                    let mut n = is_next;
                    if ui.add_enabled(!busy && e.runtime == Games::SteamVr, eframe::egui::Checkbox::new(&mut n, "Next Play is for this game")).changed() {
                        app.core.set_next_game(n.then_some(e.appid.as_str()));
                    }
                });
                if !e.why.is_empty() {
                    note(ui, &e.why);
                }
            }
        },
    );
}

/// The card.
pub fn show(ui: &mut Ui, app: &mut PimaxApp) {
    runtimes(ui, app);
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
            if widgets::ibutton(ui, icon::ARROWS_CLOCKWISE, "Check VR games", !busy).on_hover_text("omarchyvr vr game-check: takes a few seconds").clicked() {
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
