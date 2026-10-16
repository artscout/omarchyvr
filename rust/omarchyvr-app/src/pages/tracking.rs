//! Tracking: the head (6DoF), the controllers, the camera hands (developer mode), the eyes; the tests and the live
//! state in developer mode.
use crate::app::PimaxApp;
use crate::pages::{editable, tool_button, two_columns};
use crate::theme::{self, icon};
use crate::widgets::{self, chip, section, switch, switch_row};
use eframe::egui::{RichText, Ui};
use omarchyvr_gui_core::monlog::TrackState;
use omarchyvr_gui_core::tools::ToolId;

/// What the switches of the next start allow together: no controller tracking or hands without 6DoF, no hand
/// roles without hands.
pub fn rules(app: &mut PimaxApp) {
    let t = &mut app.core.toggles;
    if !t.sixdof {
        t.ctrl_track = false;
        t.hands = false;
    }
    if !t.hands {
        t.hands_roles = false;
    }
}

/// Draw the page.
pub fn show(ui: &mut Ui, app: &mut PimaxApp) {
    two_columns(ui, 380.0, |i, ui| {
        if i == 0 {
            head(ui, app);
            controllers(ui, app);
            if app.core.state.dev_mode {
                hands(ui, app);
            }
        } else {
            eyes(ui, app);
        }
    });
    rules(app);
    if app.core.state.dev_mode {
        tests(ui, app);
    }
}

fn head(ui: &mut Ui, app: &mut PimaxApp) {
    let ed = editable(app);
    let caps = app.core.facts.caps.clone();
    let t = app.core.toggles;
    section(ui, "Head", None, |ui| {
        switch_row(ui, &mut app.core.toggles.sixdof, "6DoF", Some("Inside-out head tracking with the headset cameras (SLAM). Start it in a lit room with the cameras free (LESSONS row 13)."), ed && (caps.sixdof || t.sixdof));
        if app.core.state.dev_mode {
            switch_row(ui, &mut app.core.toggles.anchor, "Room anchor", Some("The AprilTag and the room map place the world (--anchor)."), ed && (caps.anchor || t.anchor));
        }
        if app.core.live.service.is_some() {
            let p = theme::pal(ui);
            let gyro = if app.core.live.log.gyro_ready { "calibrated" } else { "calibrating" };
            widgets::setting(ui, "Gyro", None, |ui| {
                ui.label(RichText::new(gyro).size(12.0).color(p.dim));
            });
            if app.core.live.log.camera.light_low {
                widgets::status(ui, omarchyvr_gui_core::monlog::LIGHT_LOW_TEXT, p.warn);
            }
        }
    });
}

fn controllers(ui: &mut Ui, app: &mut PimaxApp) {
    let ed = editable(app);
    let caps = app.core.facts.caps.clone();
    let t = app.core.toggles;
    section(ui, "Controllers", None, |ui| {
        if app.core.state.dev_mode {
            widgets::setting(ui, "Optical tracking", Some("The controllers' position from the headset cameras (--ctrl-track; needs 6DoF). Experimental."), |ui| {
                chip(ui, "beta", theme::pal(ui).warn);
                switch(ui, &mut app.core.toggles.ctrl_track, "Optical controller tracking", ed && ((caps.ctrl_track && app.core.toggles.sixdof) || t.ctrl_track))
            });
        }
        let mut use_ctrl = app.core.wayvr.settings.controllers_in_work;
        let r = switch_row(
            ui,
            &mut use_ctrl,
            "Controllers in Work",
            Some("Off (default): with camera hands on, your bare hands are the pointer in Work (pinch = click) and the controllers are ignored entirely: rays, buttons, sticks and grabs. On: controllers work too (a tracked controller wins). Voice: \"use controllers\" / \"hands only\". Applies at once to a running session."),
            ed && app.core.toggles.hands,
        );
        if r.changed() {
            app.core.set_controllers_in_work(use_ctrl);
        }
    });
}

/// Camera hands (developer mode).
fn hands(ui: &mut Ui, app: &mut PimaxApp) {
    let ed = editable(app);
    let t = app.core.toggles;
    let hands_why = app.core.hands_unavailable();
    section(ui, "Hands", None, |ui| {
        let r = widgets::setting(ui, "Camera hands", Some("Hand tracking with the front cameras (needs 6DoF). Experimental."), |ui| {
            chip(ui, "beta", theme::pal(ui).warn);
            switch(ui, &mut app.core.toggles.hands, "Camera hands", ed && app.core.toggles.sixdof && (hands_why.is_none() || t.hands))
        });
        if r.changed() {
            app.core.state.hands_chosen = true;
        }
        if let Some(w) = &hands_why {
            r.on_disabled_hover_text(w.as_str());
        }
        switch_row(ui, &mut app.core.toggles.hands_roles, "Hands as controllers", Some("--hands-roles: apps that only read controllers see your hands (pinch = trigger); put the controllers down."), ed && app.core.toggles.hands);
        if let Some(w) = &hands_why {
            widgets::status(ui, w, theme::pal(ui).faint);
        }
    });
}

/// Eye tracking: on / off, the state, the gaze dot; calibration, refit and the live numbers in developer mode.
fn eyes(ui: &mut Ui, app: &mut PimaxApp) {
    let ed = editable(app);
    let caps = app.core.facts.caps.clone();
    let t = app.core.toggles;
    let dev = app.core.state.dev_mode;
    let (status, status_ok) = app.core.eye_status();
    section(ui, "Eyes", None, |ui| {
        let p = theme::pal(ui);
        switch_row(ui, &mut app.core.toggles.eye, "Eye tracking", Some("Visual only: a small dot where you look in Work. Your eyes never click, point, scroll or pick a window. It starts with every session once calibrated; if it cannot start, VR keeps running without it."), ed && (caps.eye || caps.no_eye || t.eye));
        widgets::status(ui, &status, if status_ok { p.ok } else { p.warn });
        let mut dot = app.core.eye_settings.dot;
        if switch_row(ui, &mut dot, "Gaze dot", Some("The dot where you look, in Work."), app.core.toggles.eye).changed() {
            app.core.set_eye_dot(dot);
        }
        if !dev {
            return;
        }
        let mut targeting = app.core.eye_settings.targeting;
        if switch_row(ui, &mut targeting, "Gaze targets windows", Some("Developer: voice commands may use the window you look at (next desk start)."), true).changed() {
            app.core.set_eye_targeting(targeting);
        }
        // the laser that shows where you look, in the running session
        let wayvr = app.core.live.alive("wayvr") || (app.core.demo && app.core.live.service.is_some());
        let mut laser = app.core.eye_laser.unwrap_or(false);
        let r = switch_row(ui, &mut laser, "Eye laser", Some("Shows where you look in the running desk session (wayvrctl gaze-laser on|off)."), wayvr && !app.core.busy());
        let r = if wayvr { r } else { r.on_disabled_hover_text("Start desk mode first (WayVR shows the laser)") };
        if r.changed() {
            app.core.set_eye_laser(laser);
        }
        ui.add_space(4.0);
        widgets::caption(ui, "Calibration");
        let acc = app.core.facts.eye_accuracy_deg;
        widgets::status(ui, &match (app.core.facts.eye_calibration, acc) {
            (true, Some(a)) => format!("Saved, validated accuracy {a:.2} deg"),
            (true, None) => "Saved (no validation figure)".into(),
            _ => "None saved yet: Calibrate (its own VR session, about 2 minutes).".into(),
        }, p.dim);
        ui.horizontal_wrapped(|ui| {
            tool_button(ui, app, ToolId::EyeCalibration, "Calibrate");
            tool_button(ui, app, ToolId::EyeView, "Eye view");
            tool_button(ui, app, ToolId::EyePowerOn, "Power on eye module");
        });
        if let Some(run) = app.core.facts.manual.eyecal_runs.first() {
            let name = run.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            let busy = app.core.busy();
            let blocker = app.core.eyecal_v2_blocker();
            ui.horizontal_wrapped(|ui| {
                if widgets::ibutton(ui, icon::ARROWS_CLOCKWISE, "Refit the newest run", !busy)
                    .on_hover_text(format!("Fit the newest saved run ({name}) again with the sample-level model, no headset needed: the table goes to the output panel."))
                    .clicked()
                {
                    app.core.eyecal_refit(false);
                }
                let r = widgets::ibutton(ui, icon::CHECK_CIRCLE, "Save the refit...", !busy && blocker.is_none());
                let r = match &blocker {
                    Some(b) => r.on_disabled_hover_text(b.as_str()),
                    None => r.on_hover_text("Writes it as your calibration (the old file is kept)"),
                };
                if r.clicked() {
                    app.core.eyecal_refit(true);
                }
            });
            if let Some(b) = blocker {
                widgets::status(ui, &b, p.warn);
            }
        }
        let eye = &app.core.live.log.eye;
        if let (true, Some(s)) = (eye.enabled, eye.stats) {
            widgets::status(ui, &format!("Now: {:.0} % valid of {} gaze queries, chip {:.0} samples/s", s.valid_pct(), s.queries, s.chip_hz), p.dim);
        }
    });
}

/// Developer mode: the live state, the tests, the guided hand test, the controller gyros.
fn tests(ui: &mut Ui, app: &mut PimaxApp) {
    let live = app.core.live.clone();
    section(ui, "Tests", None, |ui| {
        let p = theme::pal(ui);
        let six = &live.log.sixdof;
        let gyro = if live.service.is_none() { "-" } else if live.log.gyro_ready { "calibrated" } else { "calibrating" };
        let state = match (live.service.is_some(), six.state) {
            (false, _) => "nothing runs".to_string(),
            (true, TrackState::Tracking) => format!("6DoF tracking: {} acquired, {} lost, {} resets", six.acquisitions, six.losses, six.resets),
            (true, TrackState::Lost) => "6DoF lost (3DoF, position held)".to_string(),
            (true, TrackState::Waiting) => "waiting for the tracker (3DoF)".to_string(),
            (true, TrackState::Off) => "3DoF".to_string(),
        };
        widgets::status(ui, &format!("Now: {state}; gyro {gyro}"), p.dim);
        ui.horizontal_wrapped(|ui| {
            tool_button(ui, app, ToolId::WalkTest, "6DoF walk test");
            tool_button(ui, app, ToolId::CtrlRecording, "Record controllers");
            tool_button(ui, app, ToolId::HandsTest, "Hands test");
        });
        hand_test(ui, app);
        if !app.core.facts.manual.ctrl_imu_logs.is_empty() {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                widgets::caption(ui, "Controller gyros");
                widgets::help(ui, "The tracker saves its gyro fit every 30 s and at shutdown; this fits the last session's logs again (pimax-ctrltrack imufit).");
            });
            let busy = app.core.busy();
            ui.horizontal_wrapped(|ui| {
                if widgets::ibutton(ui, icon::PULSE, "Fit controller gyros", !busy).clicked() {
                    app.core.imufit(false);
                }
                if widgets::ibutton(ui, icon::CHECK_CIRCLE, "Save the fit...", !busy).clicked() {
                    app.core.imufit(true);
                }
            });
        }
    });
}

/// The guided hand test (in the headset) and its last result.
fn hand_test(ui: &mut Ui, app: &mut PimaxApp) {
    let p = theme::pal(ui);
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        widgets::caption(ui, "Guided hand test");
        widgets::help(ui, "Restarts desk mode with 6DoF and camera hands, then shows 4 steps of 10 s in the headset (left hand, right hand, both, open/close and pinch) while it records the cameras into ~/src/cam-data. Then it measures the detection rate.");
    });
    let blockers = app.core.hand_test_blockers();
    let r = widgets::ibutton(ui, icon::HAND, "Start the hand test", blockers.is_empty());
    let r = if blockers.is_empty() { r } else { r.on_disabled_hover_text(blockers.join("; ")) };
    if r.clicked() {
        app.core.start_hand_test();
    }
    if !blockers.is_empty() && !app.core.busy() {
        widgets::status(ui, &format!("Not now: {}", blockers.join("; ")), p.faint);
    }
    if let Some(res) = app.core.play.hand_test.result.clone() {
        let (text, c) = match &res.detection {
            Some(d) => (format!("Detection: {d}"), p.ok),
            None => ("No detection rate".to_string(), p.warn),
        };
        ui.label(RichText::new(text).size(12.5).color(c));
        for l in &res.live {
            ui.add(eframe::egui::Label::new(RichText::new(l).size(11.0).monospace().color(p.dim)).truncate());
        }
        if let Some(n) = &res.note {
            widgets::status(ui, n, p.warn);
        }
        if let Some(f) = &app.core.play.hand_test.file {
            ui.label(RichText::new(format!("Recording: {}", f.display())).size(11.0).color(p.faint));
        }
    }
}
