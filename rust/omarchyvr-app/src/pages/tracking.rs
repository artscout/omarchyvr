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
            stability(ui, app);
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
            let hint = omarchyvr_gui_core::monlog::gyro_hint(&app.core.live.log);
            let gyro = if hint.is_some() { "not measured yet" } else if app.core.live.log.gyro_ready { "calibrated" } else { "calibrating" };
            widgets::setting(ui, "Gyro", None, |ui| {
                ui.label(RichText::new(gyro).size(12.0).color(p.dim));
            });
            if let Some(h) = hint {
                widgets::status(ui, h, p.warn);
            }
            if app.core.live.log.camera.light_low {
                widgets::status(ui, omarchyvr_gui_core::monlog::LIGHT_LOW_TEXT, p.warn);
            }
            let sd = &app.core.live.log.sixdof;
            if let Some(total) = sd.drift_total_cm {
                let text = format!(
                    "Head tracking slid {:.0} cm without movement; held ({:.0} cm this session)",
                    sd.drift_last_cm.unwrap_or(0.0),
                    total
                );
                widgets::status(ui, &text, if total >= 50.0 { p.warn } else { p.dim });
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
        let mut steady = app.core.wayvr.settings.click_steady_px > 0.0;
        if switch_row(ui, &mut steady, "Steady clicks", Some("A click stays where you pressed until the laser moves clearly. Stops shaky clicks turning into drags. Takes effect at the next Work start."), true).changed() {
            app.core.set_click_steady(steady);
        }
        ctrl_angle(ui, app);
    });
}

/// The controller angle slider (-20..+70 deg, default 30): saved for the next start and applied live to a running
/// session (docs/CONTROLLERS-INHAND.md).
fn ctrl_angle(ui: &mut Ui, app: &mut PimaxApp) {
    use omarchyvr_config::ctrl_angle::{DEFAULT_DEG, MAX_DEG, MIN_DEG};
    let p = theme::pal(ui);
    let can = !app.core.busy() && (app.core.facts.caps.tracking || app.core.demo);
    let mut deg = app.core.toggles.ctrl_angle;
    let r = widgets::setting(
        ui,
        "Controller angle",
        Some("How far the controller pointer (and the hand on it) is tilted up from the controller's body, in degrees. Raise it if the ray points lower than you aim, lower it if it points higher. Default 30. Changes a running session within a second and is kept for the next start."),
        |ui| {
            ui.horizontal(|ui| {
                let r = ui.add_enabled(can, eframe::egui::Slider::new(&mut deg, MIN_DEG..=MAX_DEG).suffix(" deg"));
                if deg != DEFAULT_DEG && widgets::tbutton(ui, "Default", can).clicked() {
                    deg = DEFAULT_DEG;
                    return (true, true);
                }
                (r.changed(), r.drag_stopped() || (r.changed() && !r.dragged()))
            })
            .inner
        },
    );
    let (changed, done) = r;
    if changed || done {
        app.core.set_ctrl_angle(deg, done);
    }
    let (text, warn) = app.core.ctrl_angle_status();
    widgets::status(ui, &text, if warn { p.warn } else { p.dim });
}

/// The tracking switches of plan phase 1 (docs/TRACKING-STABILITY.md), all on by default; each with its status.
fn stability(ui: &mut Ui, app: &mut PimaxApp) {
    use omarchyvr_gui_core::trackset::Switch;
    let can = !app.core.busy() && (app.core.facts.caps.tracking || app.core.demo);
    section(ui, "Tracking stability", Some("Improvements of 2026-10-17, all on by default. Switch one off only if tracking got worse with it; it takes effect at the next start."), |ui| {
        let p = theme::pal(ui);
        for sw in Switch::ALL {
            let mut on = sw.get(&app.core.toggles);
            if switch_row(ui, &mut on, sw.label(), Some(sw.help()), can).changed() {
                app.core.set_tracking_switch(sw, on);
            }
            let (text, warn) = app.core.tracking_status(sw);
            widgets::status(ui, &text, if warn { p.warn } else { p.dim });
        }
        if !app.core.facts.caps.tracking && !app.core.demo {
            widgets::status(ui, "The installed omarchyvr launcher does not know these settings yet: install the new build.", p.warn);
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
        hand_models(ui, app);
    });
}

/// The hand models: not shipped (no licence), downloaded by the user after reading the notice. Shown until they are installed.
pub(crate) fn hand_models(ui: &mut Ui, app: &mut PimaxApp) {
    let p = theme::pal(ui);
    let running = app.core.hand_models_running();
    if app.core.facts.hands_models && !running && app.core.hand_models.result.is_none() {
        return;
    }
    let busy = app.core.busy();
    widgets::setting(
        ui,
        "Hand models",
        Some("Camera hands need two small model files from Monado's public server. They have no licence, so omarchyvr does not ship them: you download them to this PC yourself. docs/HANDS.md."),
        |ui| {
            if app.core.facts.hands_models {
                widgets::chip(ui, "installed", p.ok);
            } else if running {
                widgets::spinner(ui);
            } else if !app.core.hand_models.asking && widgets::primary(ui, "Download hand models", !busy).clicked() {
                app.core.hand_models_ask();
            }
        },
    );
    if app.core.hand_models.asking {
        widgets::note(ui, omarchyvr_gui_core::handmodels::CONSENT);
        widgets::note(ui, "It needs git and git-lfs on this PC and a few minutes of internet. Nothing needs your password.");
        ui.horizontal(|ui| {
            if widgets::primary(ui, "I agree, download", !busy).clicked() {
                app.core.hand_models_download();
            }
            if widgets::ghost(ui, icon::X_CIRCLE, "Cancel", true).clicked() {
                app.core.hand_models_cancel();
            }
        });
    }
    if running {
        let line = app.core.hand_models_progress().unwrap_or_else(|| "Starting the download...".into());
        widgets::status(ui, &line, p.dim);
    }
    match app.core.hand_models.result.clone() {
        Some(Ok(m)) => widgets::status(ui, &m, p.ok),
        Some(Err(e)) => widgets::status(ui, &e, p.fail),
        None => {}
    }
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
        switch_row(ui, &mut app.core.toggles.eye, "Eye tracking", Some("A small dot where you look in Work. By default your eyes never click, point, move the mouse or pick a window. It starts with every session (with the built-in test calibration until you calibrate your own); if it cannot start, VR keeps running without it."), ed && (caps.eye || caps.no_eye || t.eye));
        widgets::status(ui, &status, if status_ok { p.ok } else { p.warn });
        let mut dot = app.core.eye_settings.dot;
        if switch_row(ui, &mut dot, "Gaze dot", Some("The dot where you look, in Work."), app.core.toggles.eye).changed() {
            app.core.set_eye_dot(dot);
        }
        // the ONE gaze input switch (user decision 2026-10-17): off = the gaze is only a visual point
        let mut mouse = app.core.eye_settings.mouse;
        if switch_row(ui, &mut mouse, "Eyes as a mouse", Some("Your eyes move the pointer and can click. Off (default): your eyes only show a dot. Takes effect at the next Work start."), app.core.toggles.eye).changed() {
            app.core.set_eye_mouse(mouse);
        }
        // learning from clicks: never input; the quick check and "forget" talk to the running session
        let mut learn = app.core.eye_settings.learn;
        if switch_row(ui, &mut learn, "Learn from my clicks", Some("Each click teaches the eye tracker where you really looked. Your eyes still never click unless Eyes as a mouse is on. Takes effect at the next Work start."), app.core.toggles.eye).changed() {
            app.core.set_eye_learn(learn);
        }
        let work_runs = app.core.live.alive("wayvr") || app.core.demo;
        if work_runs {
            app.core.eye_learn_refresh();
        }
        let learn_text = match (app.core.eye_learn_text(), work_runs) {
            (Some(t), _) => t,
            (None, true) => "Asking the session...".into(),
            (None, false) => "Start Work to see what it learned".into(),
        };
        widgets::status(ui, &learn_text, p.dim);
        ui.horizontal_wrapped(|ui| {
            let busy = app.core.task_busy("gazecheck");
            let r = widgets::ibutton(ui, icon::CHECK_CIRCLE, "Quick eye check", work_runs && !busy && app.core.toggles.eye);
            let r = if work_runs { r.on_hover_text("Shows a dot in front of you in the headset: look at it for a moment. Takes up to 8 seconds.") } else { r.on_disabled_hover_text("Start Work first") };
            if r.clicked() {
                app.core.eye_quick_check();
            }
            let r = widgets::ibutton(ui, icon::ARROWS_CLOCKWISE, "Forget what it learned", work_runs && !app.core.task_busy("gazelearn"));
            let r = if work_runs { r.on_hover_text("Throws away the lessons of the running session.") } else { r.on_disabled_hover_text("Start Work first") };
            if r.clicked() {
                app.core.eye_learn_forget();
            }
        });
        if !work_runs {
            widgets::status(ui, "Start Work first", p.dim);
        }
        if let Some((t, ok)) = app.core.eye_check.clone() {
            widgets::status(ui, &t, if ok { p.ok } else { p.warn });
        }
        // which start list and which calibration: in plain words, with the switch between the built-in test calibration and yours
        let own = app.core.facts.eye_calibration;
        let mut builtin = app.core.eye_cal_source() == omarchyvr_config::eye::CalSource::Builtin;
        let was_builtin = builtin;
        let r = switch_row(
            ui,
            &mut builtin,
            "Built-in test calibration",
            Some("On: the calibration of the developer's eyes (someone else's, so the aim is rough). Off: your own, made with the eye calibration step in Onboarding. Once you have calibrated, yours is used by default."),
            app.core.toggles.eye && (own || !was_builtin),
        );
        if r.changed() {
            app.core.set_eye_calibration(if builtin { omarchyvr_config::eye::CalMode::Builtin } else { omarchyvr_config::eye::CalMode::Own });
        }
        widgets::status(ui, &app.core.eye_sources_text(), if builtin { p.warn } else { p.dim });
        if !dev {
            return;
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
