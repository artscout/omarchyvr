//! Room setup, the third big button (docs/ROOM-SETUP.md): one guided flow. (a) the headset's factory calibration is
//! there, or it is read from the headset (an asleep headset gets the wake guide), (b) the room setup (Fast by default,
//! Full as an option; the steps show in the headset), (c) a short summary with ticks. The state machine is
//! `omarchyvr_gui_core::roomflow`; this file only draws it.
use crate::app::PimaxApp;
use crate::theme::{self, icon};
use crate::widgets::{self, chip, inset, note, title};
use eframe::egui::{self, RichText, Ui};
use omarchyvr_gui_core::app::Page;
use omarchyvr_gui_core::roomflow::Stage;

/// The third big tile (Home and the Ready screen): the button and its state.
pub fn tile(ui: &mut Ui, app: &mut PimaxApp, width: f32) {
    app.core.want_room();
    let p = theme::pal(ui);
    let busy = app.core.busy();
    let (done, words) = app.core.room_tile();
    if super::home::big_choice(ui, icon::MAP_TRIFOLD, "Room setup", "Calibrate and map your room", !busy, width)
        .on_hover_text("One guided flow: reads your headset's calibration if it is missing, then maps your room in the headset (Fast, 30 seconds, or Full), then shows what was done.")
        .clicked()
    {
        app.core.open_room_flow();
    }
    ui.horizontal(|ui| {
        widgets::dot(ui, if done { p.ok } else { p.warn }, 3.0);
        ui.label(RichText::new(words).size(11.5).color(if done { p.ok } else { p.dim }));
    });
}

/// Draw the page.
pub fn show(ui: &mut Ui, app: &mut PimaxApp) {
    let p = theme::pal(ui);
    widgets::card(ui, |ui| {
        widgets::title_row(ui, "Room setup", None, |ui| {
            if widgets::ghost(ui, icon::X_CIRCLE, "Close", true).on_hover_text("Back to Home (a running step keeps going)").clicked() {
                app.core.page = Page::Desk;
            }
        });
        ui.add(egui::Label::new(RichText::new("Three short steps: your headset's calibration, your room, and a summary. The room steps are guided inside the headset.").size(13.0).color(p.dim)).wrap());
        ui.add_space(6.0);
        stepper(ui, app);
    });
    ui.add_space(10.0);
    if let Some(m) = Some(app.core.message.clone()).filter(|m| !m.is_empty()) {
        widgets::status(ui, &m, p.warn);
    }
    match app.core.room_flow.stage.clone() {
        Stage::Closed => {
            app.core.open_room_flow();
        }
        Stage::NeedCalibration => need_calibration(ui, app, None),
        Stage::ReadFailed(m) => need_calibration(ui, app, Some(&m)),
        Stage::Asleep => asleep(ui, app),
        Stage::Reading => reading(ui, app),
        Stage::Choose => choose(ui, app),
        Stage::Mapping { full } => mapping(ui, app, full),
        Stage::Summary => summary(ui, app),
    }
}

/// 1 Calibration - 2 Room - 3 Summary.
fn stepper(ui: &mut Ui, app: &PimaxApp) {
    let p = theme::pal(ui);
    let cur = match app.core.room_flow.stage {
        Stage::NeedCalibration | Stage::ReadFailed(_) | Stage::Asleep | Stage::Reading | Stage::Closed => 0,
        Stage::Choose | Stage::Mapping { .. } => 1,
        Stage::Summary => 2,
    };
    ui.horizontal_wrapped(|ui| {
        for (i, l) in ["1  Headset calibration", "2  Your room", "3  Summary"].iter().enumerate() {
            if i > 0 {
                ui.label(RichText::new("\u{2014}").size(12.0).color(p.faint));
            }
            chip(ui, l, if i == cur { p.accent } else if i < cur { p.ok } else { p.faint });
        }
    });
}

fn line(ui: &mut Ui, mark: &str, color: egui::Color32, text: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(mark).color(color));
        ui.label(RichText::new(text).strong().color(theme::pal(ui).text));
    });
}

fn need_calibration(ui: &mut Ui, app: &mut PimaxApp, failed: Option<&str>) {
    let p = theme::pal(ui);
    widgets::card(ui, |ui| {
        title(ui, "Headset calibration");
        note(ui, "Your headset carries its own factory calibration (cameras, motion sensor, lenses). 6DoF tracking needs it. It is read once from the headset, kept on this PC only, and kept through a reinstall.");
        ui.add_space(6.0);
        match failed {
            Some(m) => {
                line(ui, icon::X_CIRCLE, p.warn, "Could not read it");
                note(ui, m);
            }
            None => line(ui, icon::CIRCLE, p.warn, "Not on this PC yet"),
        }
        ui.add_space(6.0);
        let can = !app.core.busy();
        if widgets::primary(ui, if failed.is_some() { "Try again" } else { "Read it from the headset" }, can).clicked() {
            app.core.room_flow_read();
        }
    });
}

fn asleep(ui: &mut Ui, app: &mut PimaxApp) {
    let p = theme::pal(ui);
    widgets::card(ui, |ui| {
        title(ui, "Wake the headset first");
        note(ui, "The headset is asleep, so its calibration cannot be read yet. Follow the steps; this page reads it by itself the moment the headset wakes up.");
        ui.add_space(6.0);
        super::onboarding::wake_guide(ui, app, false);
        ui.add_space(6.0);
        ui.label(RichText::new("Nothing is sent to the headset while it sleeps.").size(11.5).color(p.faint));
        if widgets::tbutton(ui, "Back", true).clicked() {
            app.core.room_flow_restart();
        }
    });
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
}

fn reading(ui: &mut Ui, _app: &mut PimaxApp) {
    widgets::card(ui, |ui| {
        title(ui, "Reading the calibration from the headset");
        ui.horizontal(|ui| {
            widgets::spinner(ui);
            ui.label(RichText::new("About 10 seconds. It only reads; nothing on the headset is changed.").size(12.5).color(theme::pal(ui).dim));
        });
    });
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
}

fn choose(ui: &mut Ui, app: &mut PimaxApp) {
    let p = theme::pal(ui);
    widgets::card(ui, |ui| {
        title(ui, "Your room");
        line(ui, icon::CHECK_CIRCLE, p.ok, "Headset calibration is in place");
        ui.add_space(6.0);
        note(ui, "The steps show in the headset: put it on. If Work is not running, it starts first. Turn the room lights on, the cameras need light.");
        ui.add_space(6.0);
        inset(ui, |ui| {
            ui.label(RichText::new("Fast setup, about 30 seconds").strong().color(p.text));
            note(ui, "Stand in the middle of your play space and look around slowly. Then look forward and pull a trigger (or hold still).");
            let b = app.core.room_setup_blockers(false);
            let can = b.is_empty() && !app.core.busy();
            let r = widgets::primary(ui, "Start Fast setup", can);
            let r = if can { r } else { r.on_disabled_hover_text(if b.is_empty() { "Another action is running".to_string() } else { b.join(" ") }) };
            if r.clicked() {
                app.core.room_flow_choose(false);
            }
            if !b.is_empty() {
                widgets::status(ui, &b.join(" "), p.warn);
            }
        });
        ui.add_space(6.0);
        inset(ui, |ui| {
            ui.label(RichText::new("Full setup, about 3 minutes").strong().color(p.text));
            note(ui, "Walk the whole room, lay a controller on the floor, then trace the edge of your free space holding a trigger.");
            let can = app.core.room_setup_blockers(true).is_empty() && !app.core.busy();
            if widgets::tbutton(ui, "Start Full setup", can).clicked() {
                app.core.room_flow_choose(true);
            }
        });
    });
}

fn mapping(ui: &mut Ui, app: &mut PimaxApp, full: bool) {
    let p = theme::pal(ui);
    widgets::card(ui, |ui| {
        title(ui, if full { "Full room setup is running" } else { "Fast room setup is running" });
        line(ui, icon::CHECK_CIRCLE, p.ok, "Headset calibration is in place");
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            widgets::spinner(ui);
            ui.label(RichText::new("Put the headset on. The steps are shown and spoken inside the headset.").size(12.5).color(p.dim));
        });
        if let Some(j) = &app.core.job {
            ui.label(RichText::new(&j.label).size(11.5).color(p.faint));
        }
    });
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
}

fn summary(ui: &mut Ui, app: &mut PimaxApp) {
    let p = theme::pal(ui);
    let ok = app.core.room_flow.room_ok == Some(true);
    let ticks = {
        let f = app.core.room_flow.clone();
        let cal = app.core.calibration_ok();
        f.ticks(&Env { cal, usb: app.core.facts.usb.clone() })
    };
    widgets::card(ui, |ui| {
        title(ui, if ok { "Done" } else { "The room setup did not finish" });
        for (t, done) in &ticks {
            line(ui, if *done { icon::CHECK_CIRCLE } else { icon::X_CIRCLE }, if *done { p.ok } else { p.warn }, t);
        }
        ui.add_space(6.0);
        if ok {
            let status = omarchyvr_gui_core::roomsetup::status_line(&app.core);
            note(ui, &status);
        } else {
            note(ui, "The reason is in the Output panel. Run it again when you are ready; nothing was lost.");
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if widgets::primary(ui, "Back to Home", true).clicked() {
                app.core.page = Page::Desk;
            }
            if widgets::tbutton(ui, "Do it again", !app.core.busy()).clicked() {
                app.core.room_flow_restart();
            }
        });
    });
}

/// The flow's view of the PC for the summary ticks.
struct Env {
    cal: bool,
    usb: Vec<(u16, u16)>,
}

impl omarchyvr_gui_core::roomflow::FlowEnv for Env {
    fn calibration_present(&self) -> bool {
        self.cal
    }
    fn chip(&self) -> omarchyvr_gui_core::headset_choice::HeadsetUsb {
        omarchyvr_gui_core::headset_choice::headset_state_from_usb(&self.usb)
    }
}
