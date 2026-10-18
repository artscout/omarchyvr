//! WayVR on the Desktop page (developer mode): subtitles, live actions over `wayvrctl` (reset, show/hide, panels,
//! apps), the settings of the next desk start (`~/.config/omarchyvr/wayvr-desk.conf`, applied by the launcher), the
//! conf.d check, the saved screen layout and the controls guide; screen numbers and the captions screen for every
//! mode.
use crate::app::PimaxApp;
use crate::theme::{self, icon};
use crate::widgets::{self, chip, note, section, switch_row};
use eframe::egui::{self, RichText, Ui};
use omarchyvr_gui_core::wayvr::{Ctl, LintLevel, ScreenNumbers};

/// The WayVR cards of the Desktop page (developer mode).
pub fn body(ui: &mut Ui, app: &mut PimaxApp) {
    app.core.want_wayvr();
    let p = theme::pal(ui);
    let running = app.core.live.alive("wayvr") || (app.core.demo && app.core.live.service.is_some());
    section(ui, "Subtitles", Some("The same switch as the voice commands \"captions on\" and \"captions off\". Play has no WayVR panel, so subtitles are a Work feature."), |ui| {
        let busy = app.core.busy();
        match app.core.subtitles_live() {
            Some(on) => {
                let mut v = on;
                if switch_row(ui, &mut v, "Subtitles now", Some("What you say, under the main screen of the running desk."), !busy).changed() {
                    app.core.set_subtitles(v);
                }
            }
            None => widgets::status(ui, "No desk session runs: the switch below decides the next start.", p.faint),
        }
        let mut d = app.core.toggles.voice;
        let can = app.core.facts.caps.voice || d;
        if switch_row(ui, &mut d, "At desk start", Some("Subtitles on when the desk starts (--voice)."), can).changed() {
            app.core.set_subtitles_default(d);
        }
    });
    section(ui, "WayVR now", None, |ui| {
        if !running {
            note(ui, "WayVR is not running: start desk mode for the live controls.");
        }
        ui.add_enabled_ui(running, |ui| {
            ui.horizontal_wrapped(|ui| {
                for c in [Ctl::ResetScreens, Ctl::SwitchSet(1), Ctl::SwitchSet(0), Ctl::ShowHide] {
                    let ic = match c {
                        Ctl::ResetScreens => icon::ARROW_COUNTER_CLOCKWISE,
                        Ctl::SwitchSet(0) => icon::EYE_SLASH,
                        Ctl::SwitchSet(_) => icon::EYE,
                        _ => icon::SQUARES_FOUR,
                    };
                    let tip = match c {
                        Ctl::ResetScreens => "wayvrctl desk-recenter: the whole layout in front of you, yaw only, same shape. Screens never move otherwise.",
                        Ctl::SwitchSet(0) => "wayvrctl switch-set 0",
                        Ctl::SwitchSet(_) => "wayvrctl switch-set 1",
                        _ => "wayvrctl show-hide",
                    };
                    if widgets::ibutton(ui, ic, &c.label(), true).on_hover_text(tip).clicked() {
                        app.core.wayvr_ctl(c);
                    }
                }
            });
            widgets::setting(ui, "Wrist readout", None, |ui| {
                for v in [true, false] {
                    if widgets::tbutton(ui, if v { "Show" } else { "Hide" }, true).on_hover_text("panel-modify perf perf_outer set-visible (not tried in the headset yet)").clicked() {
                        app.core.wayvr_ctl(Ctl::Panel { panel: "perf", element: "perf_outer", visible: v });
                    }
                }
            });
            widgets::setting(ui, "Screen numbers", None, |ui| {
                if widgets::tbutton(ui, "Show now", true).on_hover_text("wayvrctl screen-numbers flash: the numbers 1-6 for about 5 seconds").clicked() {
                    app.core.wayvr_ctl(Ctl::FlashScreenNumbers);
                }
            });
            ui.horizontal_wrapped(|ui| {
                if widgets::ismall(ui, icon::LIST, "List apps").clicked() {
                    app.core.wayvr_ctl(Ctl::ProcessList);
                }
                let mut stop = None;
                if let Some(ps) = &app.core.wayvr.processes {
                    if ps.is_empty() {
                        ui.label(RichText::new("no apps inside WayVR").color(p.faint));
                    }
                    for (name, h) in ps {
                        ui.label(RichText::new(name).color(p.text));
                        if widgets::ismall(ui, icon::X_CIRCLE, "stop").on_hover_text("wayvrctl process-terminate (SIGTERM to the app inside WayVR)").clicked() {
                            stop = Some(*h);
                        }
                    }
                }
                if let Some(h) = stop {
                    app.core.wayvr_ctl(Ctl::Terminate(h));
                }
            });
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut app.core.wayvr.launch_exec).hint_text("program, e.g. foot").desired_width(180.0));
                let exec = app.core.wayvr.launch_exec.trim().to_string();
                if widgets::ibutton(ui, icon::PLAY, "Start inside WayVR", !exec.is_empty()).clicked() {
                    let name = exec.split_whitespace().next().unwrap_or("app").rsplit('/').next().unwrap_or("app").to_string();
                    app.core.wayvr_ctl(Ctl::Launch { name, exec });
                }
            });
        });
        if !app.core.wayvr.last.is_empty() {
            ui.label(RichText::new(&app.core.wayvr.last).size(12.0).color(p.faint));
        }
    });

    let saved_help = format!(
        "Saved to {}; omarchyvr desk start writes it as conf.d/{} (WayVR reads its config once, at start).",
        app.core.wayvr_paths.settings.display(),
        omarchyvr_gui_core::wayvr::CONF_NAME
    );
    section(ui, "Next desk start", Some(&saved_help), |ui| {
        let s = &mut app.core.wayvr.settings;
        let before = s.clone();
        switch_row(ui, &mut s.world_lock_grab, "Grab to place", Some("Grab a screen to place it in the room."), true);
        switch_row(ui, &mut s.world_lock_pin, "A / X pins a screen", None, s.world_lock_grab);
        switch_row(ui, &mut s.grab_keep_apparent_size, "Keep apparent size", Some("A screen keeps its apparent size when you push it away."), true);
        switch_row(ui, &mut s.enable_keyboard, "Keyboard", Some("The WayVR keyboard."), true);
        // gaze input lives in ONE place: Tracking > Eyes > "Eyes as a mouse" (eye.conf gaze_mouse, default off)
        switch_row(ui, &mut s.pointing_moves_cursor, "Pointing moves the cursor", Some("Off (default): pointing at a screen leaves your real desktop cursor alone; it moves when you click, press the trigger or grab."), true);
        switch_row(ui, &mut s.auto_arrange, "Auto-arrange apps", Some("Off (default): apps stay on the screen where you opened them and move only when you ask (\"move left\"). On: a new app of another kind is sent to its own screen."), true);
        switch_row(ui, &mut s.hide_grab_help, "Hide grab hint", None, true);
        switch_row(ui, &mut s.controllers_in_work, "Controllers in Work", Some("Off = hands only."), true);
        switch_row(ui, &mut s.controller_models, "Controller models", Some("A 3D controller in each hand in Work, like SteamVR shows; the pointer ray leaves its front tip."), true);
        switch_row(ui, &mut s.controller_axes, "Pointer and grip axes", Some("Debug: coloured axes on the controllers, to check the pointer in the headset."), true);
        widgets::setting(ui, "Push / pull speed", None, |ui| {
            ui.add(egui::Slider::new(&mut s.grab_dist_rate, 0.0..=3.0).step_by(0.1));
        });
        widgets::setting(ui, "Screen width", None, |ui| {
            ui.add(egui::DragValue::new(&mut s.grab_size_min).range(0.1..=10.0).speed(0.05).suffix(" m"));
            ui.label("to");
            ui.add(egui::DragValue::new(&mut s.grab_size_max).range(0.1..=20.0).speed(0.05).suffix(" m"));
        });
        widgets::setting(ui, "Distance", None, |ui| {
            ui.add(egui::DragValue::new(&mut s.grab_dist_min).range(0.1..=10.0).speed(0.05).suffix(" m"));
            ui.label("to");
            ui.add(egui::DragValue::new(&mut s.grab_dist_max).range(0.2..=30.0).speed(0.05).suffix(" m"));
        });
        let bad = s.validate();
        for b in &bad {
            ui.colored_label(p.fail, format!("{} {b}", icon::WARNING));
        }
        let dirty = app.core.wayvr.on_disk.as_ref() != Some(&app.core.wayvr.settings);
        ui.horizontal(|ui| {
            if widgets::primary(ui, "Save", dirty && bad.is_empty()).clicked() {
                app.core.save_wayvr();
            }
            if ui.button("Launcher defaults").clicked() {
                app.core.wayvr.settings = omarchyvr_gui_core::wayvr::WayvrSettings { captions_screen: app.core.wayvr.settings.captions_screen.clone(), ..Default::default() };
            }
            if app.core.wayvr.pending_restart && !dirty {
                chip(ui, "restart desk to apply", p.warn);
            }
        });
        if before != app.core.wayvr.settings {
            ui.ctx().request_repaint();
        }
        for n in &app.core.wayvr.notes {
            ui.colored_label(p.warn, n);
        }
    });
    section(ui, "Config check", None, |ui| {
        if !app.core.wayvr.loaded {
            ui.horizontal(|ui| {
                widgets::spinner(ui);
                ui.label("Reading conf.d...");
            });
        } else if app.core.wayvr.lints.is_empty() {
            ui.label(RichText::new(format!("{} conf.d looks fine", icon::CHECK_CIRCLE)).color(p.ok));
        }
        for l in &app.core.wayvr.lints {
            let c = match l.level {
                LintLevel::Error => p.fail,
                LintLevel::Warn => p.warn,
                LintLevel::Info => p.dim,
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&l.file).monospace().color(c));
                ui.add(egui::Label::new(RichText::new(&l.text).color(p.dim)).wrap());
            });
        }
        note(ui, &app.core.wayvr_paths.conf_d.display().to_string());
    });

    section(ui, "Saved screen layout", None, |ui| {
        match &app.core.wayvr.layout {
            None => note(ui, "None saved yet (WayVR saves it when desk mode stops)."),
            Some(Err(e)) => {
                ui.colored_label(p.fail, e);
            }
            Some(Ok(l)) => {
                egui::Grid::new("layout").num_columns(4).spacing([14.0, 4.0]).striped(true).show(ui, |ui| {
                    for o in l.iter().filter(|o| o.is_screen()) {
                        ui.label(RichText::new(&o.name).strong());
                        ui.label(RichText::new(&o.positioning).color(p.dim));
                        ui.label(RichText::new(o.pos.map(|p| format!("{:.2} {:.2} {:.2} m", p[0], p[1], p[2])).unwrap_or_else(|| "-".into())).monospace().size(12.0));
                        ui.label(RichText::new(o.width.map(|w| format!("{w:.2} m wide")).unwrap_or_default()).color(p.dim));
                        ui.end_row();
                    }
                });
            }
        }
        ui.horizontal_wrapped(|ui| {
            let has = matches!(app.core.wayvr.layout, Some(Ok(_)));
            if widgets::ibutton(ui, icon::COPY, "Back up", has).clicked() {
                app.core.wayvr_layout(false);
            }
            let r = widgets::ibutton(ui, icon::ARROW_COUNTER_CLOCKWISE, "Reset layout", has && !running);
            let r = if running { r.on_disabled_hover_text("Stop desk mode first: WayVR saves the layout when it exits") } else { r };
            if r.clicked() {
                app.core.wayvr_layout(true);
            }
            if widgets::ibutton(ui, icon::FOLDER_OPEN, "Backups", true).clicked() {
                let d = app.core.wayvr_paths.backups.clone();
                app.core.open_folder(&d);
            }
        });
    });

    section(ui, "Controls", None, |ui| {
        if app.core.wayvr.guide.is_empty() {
            note(ui, "Grip a screen to move it; stick up/down pushes it away or pulls it closer; both grips resize; double-click B resets all screens.");
        }
        for l in &app.core.wayvr.guide {
            ui.add(egui::Label::new(RichText::new(l).size(12.5).color(p.dim)).wrap());
        }
        if widgets::ibutton(ui, icon::BOOK_OPEN, "Full guide", true).clicked() {
            let g = app.core.wayvr_paths.guide.clone();
            app.core.open_folder(&g);
        }
    });
}

/// "Screen numbers" (Off / On demand / Always): a number (1-6) at the top centre of each VR screen. Saved at once and
/// sent to a running WayVR; on the Desktop page (all modes).
pub fn screen_numbers(ui: &mut Ui, app: &mut PimaxApp) {
    app.core.want_wayvr();
    let loaded = app.core.wayvr.loaded; // never save over a settings file that was not read yet
    let cur = app.core.wayvr.settings.screen_numbers;
    const MODES: [(ScreenNumbers, &str); 3] = [(ScreenNumbers::OnDemand, "On demand"), (ScreenNumbers::Always, "Always"), (ScreenNumbers::Off, "Off")];
    let names: Vec<&str> = MODES.iter().map(|m| m.1).collect();
    let sel = MODES.iter().position(|m| m.0 == cur).unwrap_or(0);
    let help = "A number at the top centre of each VR screen: 1 main, 2 left, 3 right, 4 up, 5 up-left, 6 up-right. Say \"Max, move the terminal from screen 2 to screen 4\". On demand: shown for about 5 seconds when a screen is added or moved, or when you talk to Max. A change applies at once in a running session.";
    if let Some(i) = widgets::seg_row(ui, "Screen numbers", Some(help), &names, sel, loaded) {
        app.core.set_screen_numbers(MODES[i].0);
    }
}

/// Where the captions panel goes (the Voice card): a screen of the saved layout, or the first one.
pub fn captions_screen(ui: &mut Ui, app: &mut PimaxApp) {
    app.core.want_wayvr();
    let screens = app.core.wayvr_screens();
    let cur = app.core.wayvr.settings.captions_screen.clone();
    let mut pick: Option<Option<String>> = None;
    let help = if screens.is_empty() { Some("The screens are known once WayVR has saved a layout (after the first desk session).") } else { None };
    widgets::setting(ui, "Captions under", help, |ui| {
        egui::ComboBox::from_id_salt("captions-screen").selected_text(cur.clone().unwrap_or_else(|| "the first screen".into())).show_ui(ui, |ui| {
            if ui.selectable_label(cur.is_none(), "the first screen").clicked() {
                pick = Some(None);
            }
            for s in &screens {
                if ui.selectable_label(cur.as_deref() == Some(s.as_str()), s).clicked() {
                    pick = Some(Some(s.clone()));
                }
            }
        });
    });
    if let Some(v) = pick {
        app.core.wayvr.settings.captions_screen = v;
        app.core.save_wayvr();
    }
}
