//! The Desktop / WayVR row: live actions over `wayvrctl` (reset, show/hide, panels, apps), the settings of the next
//! desk start (`~/.config/pimax/wayvr-desk.conf`, applied by the launcher), the conf.d check, the saved screen layout
//! and the controls guide.
use crate::app::PimaxApp;
use crate::theme::{self, icon};
use crate::widgets::{self, caption, chip, inset, note, toggle};
use eframe::egui::{self, RichText, Ui};
use pimax_gui_core::wayvr::{Ctl, LintLevel};

/// Number of screens in the saved layout (None until read).
pub fn screen_count(app: &PimaxApp) -> Option<usize> {
    match &app.core.wayvr.layout {
        Some(Ok(_)) => Some(app.core.wayvr_screens().len()),
        _ => None,
    }
}

/// The row body.
pub fn body(ui: &mut Ui, app: &mut PimaxApp) {
    app.core.want_wayvr();
    let p = theme::pal(ui);
    let running = app.core.live.alive("wayvr") || (app.core.demo && app.core.live.service.is_some());
    caption(ui, "Subtitles");
    inset(ui, |ui| {
        let busy = app.core.busy();
        match app.core.subtitles_live() {
            Some(on) => {
                let mut v = on;
                if widgets::toggle(ui, &mut v, "Subtitles now (what you say, under the main screen)", !busy).changed() {
                    app.core.set_subtitles(v);
                }
            }
            None => note(ui, "No desk session runs: the setting below decides the next start."),
        }
        let mut d = app.core.toggles.voice;
        let can = app.core.facts.caps.voice || d;
        if widgets::toggle(ui, &mut d, "Subtitles on when desk starts (--voice)", can).changed() {
            app.core.set_subtitles_default(d);
        }
        note(ui, "The same switch as the voice commands \"captions on\" and \"captions off\". VR mode has no WayVR panel, so subtitles are a desk mode feature.");
    });
    caption(ui, "Now");
    inset(ui, |ui| {
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
                        Ctl::ResetScreens => "wayvrctl reset-screens: every active screen 2.5 m in front of you (same as double-click B)",
                        Ctl::SwitchSet(0) => "wayvrctl switch-set 0",
                        Ctl::SwitchSet(_) => "wayvrctl switch-set 1",
                        _ => "wayvrctl show-hide",
                    };
                    if widgets::ibutton(ui, ic, &c.label(), true).on_hover_text(tip).clicked() {
                        app.core.wayvr_ctl(c);
                    }
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Wrist readout").color(p.dim));
                for v in [true, false] {
                    if ui.small_button(if v { "show" } else { "hide" }).on_hover_text("panel-modify perf perf_outer set-visible (not tried in the headset yet)").clicked() {
                        app.core.wayvr_ctl(Ctl::Panel { panel: "perf", element: "perf_outer", visible: v });
                    }
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

    caption(ui, "Next desk start");
    inset(ui, |ui| {
        let s = &mut app.core.wayvr.settings;
        let before = s.clone();
        toggle(ui, &mut s.world_lock_grab, "Grab screens to place them in the room", true);
        toggle(ui, &mut s.world_lock_pin, "A / X click pins a screen", s.world_lock_grab);
        toggle(ui, &mut s.grab_keep_apparent_size, "Keep the apparent size when pushing away", true);
        toggle(ui, &mut s.enable_keyboard, "WayVR keyboard", true);
        toggle(ui, &mut s.gaze_pointer, "Gaze pointer (with eye tracking)", true);
        toggle(ui, &mut s.hide_grab_help, "Hide the grab hint", true);
        toggle(ui, &mut s.controllers_in_work, "Use controllers in Work (off = hands only)", true);
        toggle(ui, &mut s.controller_models, "Show 3D controller models (like SteamVR)", true);
        toggle(ui, &mut s.controller_axes, "Debug: show pointer and grip axes on the controllers", true);
        egui::Grid::new("grab-limits").num_columns(2).spacing([14.0, 6.0]).show(ui, |ui| {
            ui.label(RichText::new("Push / pull speed").color(p.dim));
            ui.add(egui::Slider::new(&mut s.grab_dist_rate, 0.0..=3.0).step_by(0.1));
            ui.end_row();
            ui.label(RichText::new("Screen width").color(p.dim));
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut s.grab_size_min).range(0.1..=10.0).speed(0.05).suffix(" m"));
                ui.label("to");
                ui.add(egui::DragValue::new(&mut s.grab_size_max).range(0.1..=20.0).speed(0.05).suffix(" m"));
            });
            ui.end_row();
            ui.label(RichText::new("Distance").color(p.dim));
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut s.grab_dist_min).range(0.1..=10.0).speed(0.05).suffix(" m"));
                ui.label("to");
                ui.add(egui::DragValue::new(&mut s.grab_dist_max).range(0.2..=30.0).speed(0.05).suffix(" m"));
            });
            ui.end_row();
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
                app.core.wayvr.settings = pimax_gui_core::wayvr::WayvrSettings { captions_screen: app.core.wayvr.settings.captions_screen.clone(), ..Default::default() };
            }
            if app.core.wayvr.pending_restart && !dirty {
                chip(ui, "restart desk to apply", p.warn);
            }
        });
        if before != app.core.wayvr.settings {
            ui.ctx().request_repaint();
        }
        note(ui, &format!(
            "Saved to {}; pimax desk start writes it as conf.d/{} (WayVR reads its config once, at start).",
            app.core.wayvr_paths.settings.display(),
            pimax_gui_core::wayvr::CONF_NAME
        ));
        for n in &app.core.wayvr.notes {
            ui.colored_label(p.warn, n);
        }
    });

    caption(ui, "Config check");
    inset(ui, |ui| {
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

    caption(ui, "Saved screen layout");
    inset(ui, |ui| {
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

    caption(ui, "Controls");
    inset(ui, |ui| {
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

/// Where the captions panel goes (the Voice row): a screen of the saved layout, or the first one.
pub fn captions_screen(ui: &mut Ui, app: &mut PimaxApp) {
    app.core.want_wayvr();
    let p = theme::pal(ui);
    let screens = app.core.wayvr_screens();
    let cur = app.core.wayvr.settings.captions_screen.clone();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Captions under").color(p.dim));
        let mut pick: Option<Option<String>> = None;
        egui::ComboBox::from_id_salt("captions-screen")
            .selected_text(cur.clone().unwrap_or_else(|| "the first screen".into()))
            .show_ui(ui, |ui| {
                if ui.selectable_label(cur.is_none(), "the first screen").clicked() {
                    pick = Some(None);
                }
                for s in &screens {
                    if ui.selectable_label(cur.as_deref() == Some(s.as_str()), s).clicked() {
                        pick = Some(Some(s.clone()));
                    }
                }
            });
        if let Some(v) = pick {
            app.core.wayvr.settings.captions_screen = v;
            app.core.save_wayvr();
        }
    });
    if screens.is_empty() {
        note(ui, "The screens are known once WayVR has saved a layout (after the first desk session).");
    }
}
