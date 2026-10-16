//! Home, after Pimax Play: settings rows on the left (each shows its current values and expands in place), the
//! devices, the runtime, the tracking mode and the big Start/Stop buttons on the right.
use crate::app::PimaxApp;
use crate::pages::{desk, room, wayvr};
use crate::theme::{self, icon};
use crate::widgets::{self, caption, chip, device_card, inset, note, pill, row, toggle, Kind};
use eframe::egui::{self, Align, Id, Layout, RichText, ScrollArea, Ui};
use pimax_gui_core::app::{DeskPhase, Page};
use pimax_gui_core::devices::{devices, Dev, DeviceView};
use pimax_gui_core::auto::AutoCmd;
use pimax_gui_core::headsets;
use pimax_gui_core::launcher::Preset;
use pimax_gui_core::monlog::TrackState;
use pimax_gui_core::play::CapState;
use pimax_gui_core::preflight::{Gate, Preflight};
use pimax_gui_core::procs::fmt_age;
use pimax_gui_core::tools::ToolId;

/// The Room row (the ROOM SETUP button opens it).
pub fn room_row() -> Id {
    Id::new("row-room")
}

fn onoff(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

/// Draw the page.
pub fn show(ui: &mut Ui, app: &mut PimaxApp) {
    for r in std::mem::take(&mut app.ui.open_rows) {
        widgets::set_row_open(ui.ctx(), Id::new(r), true);
    }
    let w = ui.available_width();
    if w >= 980.0 {
        let left_w = (w * 0.47).clamp(440.0, 640.0);
        let h = ui.available_height();
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(egui::vec2(left_w, h), Layout::top_down(Align::Min), |ui| {
                ScrollArea::vertical().id_salt("home-left").auto_shrink([false, false]).show(ui, |ui| rows(ui, app));
            });
            ui.add_space(16.0);
            ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), h), Layout::top_down(Align::Min), |ui| {
                ScrollArea::vertical().id_salt("home-right").auto_shrink([false, false]).show(ui, |ui| right(ui, app));
            });
        });
    } else {
        ScrollArea::vertical().id_salt("home-narrow").auto_shrink([false, false]).show(ui, |ui| {
            right(ui, app);
            ui.add_space(14.0);
            rows(ui, app);
        });
    }
}

/// Can the switches change (nothing runs, no job)?
fn editable(app: &PimaxApp) -> bool {
    matches!(app.core.phase(), DeskPhase::Idle | DeskPhase::Foreign) && !app.core.busy()
}

/// A tool as a button (disabled with its reasons when it cannot start).
fn tool_button(ui: &mut Ui, app: &mut PimaxApp, t: ToolId, label: &str) {
    let a = app.core.tool_availability(t);
    let can = a.ready() && !app.core.busy();
    let b = ui.add_enabled(can, egui::Button::new(label));
    let b = if can { b } else { b.on_disabled_hover_text(a.blockers.iter().map(|x| x.text.clone()).collect::<Vec<_>>().join(" ")) };
    if b.clicked() {
        app.core.run_tool(t);
    }
}

fn rows(ui: &mut Ui, app: &mut PimaxApp) {
    ui.spacing_mut().item_spacing.y = 10.0;
    // Developer mode (the switch at the bottom): the advanced rows; off = the simple flow
    let dev = app.core.state.dev_mode;
    let t = app.core.toggles;
    let ed = editable(app);
    let caps = app.core.facts.caps.clone();
    let before = t;

    let pct = app.core.state.scale_pct;
    let qname = pimax_gui_core::quality::preset_of(pct).map(str::to_string).unwrap_or_else(|| format!("custom {pct} %"));
    let q = format!("{} Hz | {qname} | ET {} | perf {}", t.hz(), onoff(t.eye), onoff(t.perf));
    row(ui, Id::new("row-quality"), icon::PULSE, "Quality / Performance", &q, |ui| {
        caption(ui, "Refresh rate");
        ui.add_enabled_ui(ed, |ui| {
            ui.horizontal(|ui| {
                ui.radio_value(&mut app.core.toggles.hz90, false, "72 Hz");
                ui.radio_value(&mut app.core.toggles.hz90, true, "90 Hz (new)").on_hover_text(
                    "New: the headset is started at 90 Hz with exactly the commands Pimax Play sends on Windows. \
                     Applied when VR starts. Tell us if the picture looks right in the headset.",
                );
            });
        });
        caption(ui, "Render quality");
        ui.add_enabled_ui(ed, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (name, p, what) in pimax_gui_core::quality::PRESETS {
                    if ui.radio(pct == p, name).on_hover_text(format!("{}: {what}", pimax_gui_core::quality::describe(p))).clicked() && pct != p {
                        app.core.set_scale(p);
                    }
                }
            });
        });
        note(ui, &format!("Games render at {}.", pimax_gui_core::quality::describe(app.core.state.scale_pct)));
        if app.core.facts.games_steamvr {
            note(ui, "With SteamVR, SteamVR's own resolution setting multiplies this (on Auto it picks up to 150 % of the pixels on a fast GPU).");
        }
        if dev {
            caption(ui, "Custom render scale");
            let mut cp = app.core.state.scale_pct;
            let r = ui.add_enabled(ed, egui::Slider::new(&mut cp, 50..=200).step_by(5.0).custom_formatter(|v, _| format!("{v:.0} %")));
            if r.changed() {
                app.core.set_scale(cp);
            }
            note(ui, "XRT_COMPOSITOR_SCALE_PERCENTAGE: Monado's recommended size per eye (both sides scale by it); above 100 % is supersampling.");
        }
        caption(ui, "Brightness");
        {
            let cur = if app.core.facts.image_brightness.is_empty() { "bright".to_string() } else { app.core.facts.image_brightness.clone() };
            ui.add_enabled_ui(!app.core.busy(), |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (v, label, what) in pimax_gui_core::quality::IMAGES {
                        if ui.radio(cur == v, label).on_hover_text(what).clicked() && cur != v {
                            app.core.image_brightness(v);
                        }
                    }
                });
            });
            note(ui, "Image brightness for games (Bright is the default). It lifts the mid tones; black and white stay as they are, so nothing burns out. Applies to the next start.");
            let saved = if app.core.facts.backlight == 0 { 100 } else { app.core.facts.backlight };
            let mut b = app.ui.backlight_edit.unwrap_or(saved);
            ui.horizontal(|ui| {
                ui.label("Panel backlight");
                ui.spacing_mut().slider_width = 220.0;
                let r = ui.add(egui::Slider::new(&mut b, pimax_gui_core::quality::MIN_BACKLIGHT..=100).suffix(" %"));
                if r.changed() {
                    app.ui.backlight_edit = Some(b);
                }
                if (r.drag_stopped() || (r.changed() && !r.dragged())) && b != saved {
                    app.core.backlight(b);
                    app.ui.backlight_edit = None;
                }
            });
            note(ui, "100 % is the panel's maximum (it is already there on Linux). Changes the headset right away and is sent again at every Play.");
        }
        caption(ui, "While playing");
        {
            let mut on = app.core.facts.game_mode;
            let r = widgets::toggle(ui, &mut on, "Game mode: pause the local AI while playing", !app.core.busy());
            let r = r.on_hover_text("Play pauses the local voice AI (it uses the graphics card) and Stop starts it again. Sunshine is left alone unless you turn on the switch below. Work mode is not affected. Help > Render quality.");
            if r.changed() {
                app.core.game_mode(on);
            }
            let mut sun = app.core.facts.pause_sunshine;
            let r = widgets::toggle(ui, &mut sun, "Also pause Sunshine (stops game streaming)", !app.core.busy());
            let r = r.on_hover_text("Off by default: Sunshine keeps streaming while you play (it uses about 25 % of the graphics card). Turn on to pause it at Play and start it again at Stop.");
            if r.changed() {
                app.core.pause_sunshine(sun);
            }
        }
        caption(ui, "Compositor time budget");
        ui.add_enabled_ui(ed && !app.core.busy(), |ui| {
            ui.horizontal_wrapped(|ui| {
                for (v, label) in pimax_gui_core::quality::BUDGETS {
                    if ui.radio(app.core.facts.comp_budget == v, label).on_hover_text("How much time the headset compositor keeps for itself before each frame is shown. Auto = Monado's 3 ms. More starts the frame earlier: fewer late frames, a little more delay.").clicked() && app.core.facts.comp_budget != v {
                        app.core.comp_budget(v);
                    }
                }
            });
        });
        note(ui, "To compare: play the same scene for a minute per choice and look at the late % and game fps of the session card. Applies to the next start.");
        caption(ui, "Rendering");
        if dev {
            toggle(ui, &mut app.core.toggles.eye, "Eye tracking (ET)", ed && (caps.eye || t.eye));
        }
        // HIDDEN (not ready): foveated rendering is not built yet (it only showed "soon")
        // ui.horizontal(|ui| {
        //     let mut off = false;
        //     toggle(ui, &mut off, "Foveated rendering", false);
        //     soon(ui);
        // });
        if dev {
            toggle(ui, &mut app.core.toggles.perf, "Performance readout on the right wrist", ed && caps.no_perf)
                .on_hover_text("pimax-perf: HMD and app frame rate, missed frames, CPU/GPU time on a small WayVR panel (desk start without --no-perf).");
        }
        if !ed {
            note(ui, "Changes apply to the next start.");
        }
    });

    let ipd = app.core.facts.ipd_mm;
    let o = format!("IPD {} | brightness {} | AE {}", ipd.map(|m| format!("{m:.1} mm")).unwrap_or_else(|| "not set".into()), if app.core.facts.image_brightness.is_empty() { "bright" } else { app.core.facts.image_brightness.as_str() }, if t.sixdof { "auto" } else { "off" });
    row(ui, Id::new("row-optics"), icon::SUN, "Optics / Display", &o, |ui| {
        caption(ui, "IPD (interpupillary distance)");
        app.core.read_lens_ipd();
        let pal = theme::pal(ui);
        let lens = match app.core.lens_ipd {
            Some(l) => format!("Lenses now: {l:.1} mm"),
            None => "Lenses now: not read (the headset is off USB or a session is running)".to_string(),
        };
        ui.label(RichText::new(lens).size(12.0).color(pal.dim));
        let mut v = app.ui.ipd_edit.or(ipd).unwrap_or(63.0);
        let moving = app.core.moving_lenses();
        let blocker = app.core.ipd_motor_blocker();
        ui.horizontal(|ui| {
            if ui.add(egui::Slider::new(&mut v, pimax_gui_core::ipdmotor::MIN_MM..=pimax_gui_core::ipdmotor::MAX_MM).step_by(0.1).suffix(" mm")).changed() {
                app.ui.ipd_edit = Some(v);
            }
            let changed = app.ui.ipd_edit.is_some_and(|e| Some(e) != ipd);
            if widgets::primary(ui, "Apply", changed && !moving).clicked() {
                app.core.set_ipd(v);
                app.ui.ipd_edit = None;
            }
        });
        if moving {
            ui.add(egui::ProgressBar::new(0.5).desired_width(ui.available_width()).animate(true).text("Moving the lenses (about 1.8 mm per second)"));
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
        } else if let Some(r) = &app.core.ipd_result {
            ui.label(RichText::new(&r.text).size(12.0).color(if r.ok { pal.ok } else { pal.warn }));
        }
        match &blocker {
            Some(b) if !moving => note(ui, &format!("Lenses will not move: {b}")),
            _ => note(ui, "Apply moves the lenses to this value (58-72 mm, the headset's own motor) and saves it for Monado (~/.config/pimax/ipd.env), which draws with it from the next start."),
        }
        if dev {
            ui.horizontal_wrapped(|ui| tool_button(ui, app, ToolId::Ipd, "Measure with the eye tracker"));
        }
        if dev {
        caption(ui, "Camera auto exposure");
        let cam = &app.core.live.log.camera;
        note(ui, &if app.core.live.service.is_some() {
            format!("{} (camera {})", if cam.ae { "on" } else { "off" }, cam.fps.map(|f| format!("{f:.0} fps")).unwrap_or_else(|| "-".into()))
        } else {
            "On with 6DoF (the approved 0x51 exposure loop of the camera broker).".to_string()
        });
        caption(ui, "Lenses");
        ui.horizontal_wrapped(|ui| tool_button(ui, app, ToolId::LensTest, "Lens test patterns"));
        }
    });

    let live = app.core.live.clone();
    let gyro = if live.service.is_none() { "-" } else if live.log.gyro_ready { "calibrated" } else { "calibrating" };
    let tr = format!("6DoF {} | controllers {} | gyro {gyro}", onoff(t.sixdof), if t.ctrl_track { "optical" } else { "3DoF" });
    row(ui, Id::new("row-tracking"), icon::CROSSHAIR, "Tracking", &tr, |ui| {
        toggle(ui, &mut app.core.toggles.sixdof, "6DoF head tracking (inside-out, cameras + SLAM)", ed && (caps.sixdof || t.sixdof));
        let hands_why = app.core.hands_unavailable();
        if dev {
            ui.horizontal(|ui| {
                toggle(ui, &mut app.core.toggles.ctrl_track, "Optical controller tracking", ed && ((caps.ctrl_track && app.core.toggles.sixdof) || t.ctrl_track));
                chip(ui, "experimental", theme::pal(ui).warn);
            });
            toggle(ui, &mut app.core.toggles.anchor, "Room anchor (the AprilTag and the room map place the world)", ed && (caps.anchor || t.anchor));
            ui.horizontal(|ui| {
                let r = toggle(ui, &mut app.core.toggles.hands, "Camera hands (hand tracking with the front cameras)", ed && app.core.toggles.sixdof && (hands_why.is_none() || t.hands));
                if r.changed() {
                    app.core.state.hands_chosen = true;
                }
                if let Some(w) = &hands_why {
                    r.on_disabled_hover_text(w.as_str());
                }
                chip(ui, "experimental", theme::pal(ui).warn);
            });
            toggle(ui, &mut app.core.toggles.hands_roles, "Hands replace controllers (pinch = trigger)", ed && app.core.toggles.hands)
                .on_hover_text("--hands-roles: apps that only read controllers see your hands; put the controllers down.");
        }
        let mut use_ctrl = app.core.wayvr.settings.controllers_in_work;
        let r = toggle(ui, &mut use_ctrl, "Use controllers in Work", ed && app.core.toggles.hands).on_hover_text(
            "Off (default): in Work your bare hands are the pointer (pinch = click) and the controllers are ignored entirely: \
             rays, buttons, sticks and grabs. On: controllers work too (a tracked controller wins). \
             Voice: \"use controllers\" / \"hands only\". Applies at once to a running session.",
        );
        if r.changed() {
            app.core.set_controllers_in_work(use_ctrl);
        }
        if dev {
        if let Some(w) = &hands_why {
            note(ui, w);
        }
        }
        if !app.core.toggles.sixdof {
            app.core.toggles.ctrl_track = false;
            app.core.toggles.hands = false;
        }
        if !app.core.toggles.hands {
            app.core.toggles.hands_roles = false;
        }
        note(ui, "Start 6DoF in a lit room with the cameras free (LESSONS row 13).");
        if dev {
        caption(ui, "Now");
        let six = &live.log.sixdof;
        let state = match (live.service.is_some(), six.state) {
            (false, _) => "nothing runs".to_string(),
            (true, TrackState::Tracking) => format!("6DoF tracking: {} acquired, {} lost, {} resets", six.acquisitions, six.losses, six.resets),
            (true, TrackState::Lost) => "6DoF lost (3DoF, position held)".to_string(),
            (true, TrackState::Waiting) => "waiting for the tracker (3DoF)".to_string(),
            (true, TrackState::Off) => "3DoF".to_string(),
        };
        note(ui, &format!("{state}; gyro {gyro}"));
        ui.horizontal_wrapped(|ui| {
            tool_button(ui, app, ToolId::WalkTest, "6DoF walk test");
            tool_button(ui, app, ToolId::CtrlRecording, "Record controllers");
            tool_button(ui, app, ToolId::HandsTest, "Hands test");
        });
        hand_test(ui, app);
        if !app.core.facts.manual.ctrl_imu_logs.is_empty() {
            caption(ui, "Controller gyros");
            note(ui, "The tracker saves its gyro fit every 30 s and at shutdown; this fits the last session's logs again (pimax-ctrltrack imufit).");
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
        }
    });

    let acc = app.core.facts.eye_accuracy_deg;
    let ey = format!(
        "{} | {} | {}",
        if app.core.facts.eye_calibration { "calibrated" } else { "not calibrated" },
        acc.map(|a| format!("{a:.1} deg")).unwrap_or_else(|| "-".into()),
        onoff(t.eye)
    );
    if dev {
    row(ui, Id::new("row-eye"), icon::EYE, "Eye tracking", &ey, |ui| {
        let before = app.core.toggles.eye;
        toggle(ui, &mut app.core.toggles.eye, "Eye tracking on at desk start", ed && (caps.eye || caps.no_eye || t.eye))
            .on_hover_text(if caps.no_eye { "On by default; off adds --no-eye to Start Desk" } else { "Adds --eye to Start Desk" });
        if app.core.toggles.eye != before {
            app.core.save_state();
        }
        // the laser that shows where you look, in the running session
        let wayvr = app.core.live.alive("wayvr") || (app.core.demo && app.core.live.service.is_some());
        let mut laser = app.core.eye_laser.unwrap_or(false);
        let r = toggle(ui, &mut laser, "Eye laser (shows where you look)", wayvr && !app.core.busy());
        let r = if wayvr { r.on_hover_text("wayvrctl gaze-laser on|off, in the running desk session") } else { r.on_disabled_hover_text("Start desk mode first (WayVR shows the laser)") };
        if r.changed() {
            app.core.set_eye_laser(laser);
        }
        caption(ui, "Calibration");
        note(ui, &match (app.core.facts.eye_calibration, acc) {
            (true, Some(a)) => format!("~/.config/pimax/eye-calibration.toml, validated accuracy {a:.2} deg"),
            (true, None) => "~/.config/pimax/eye-calibration.toml (no validation figure)".into(),
            _ => "No calibration saved yet: run the calibration (its own VR session, about 2 minutes).".into(),
        });
        ui.horizontal_wrapped(|ui| {
            tool_button(ui, app, ToolId::EyeCalibration, "Calibrate");
            tool_button(ui, app, ToolId::EyeView, "Eye view");
            tool_button(ui, app, ToolId::EyePowerOn, "Power on eye module");
        });
        if let Some(run) = app.core.facts.manual.eyecal_runs.first() {
            let name = run.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            caption(ui, "Refit");
            note(ui, &format!("Fit the newest saved run ({name}) again with the sample-level model, no headset needed: the table goes to the output panel, Save writes it as your calibration (the old file is kept)."));
            let busy = app.core.busy();
            let blocker = app.core.eyecal_v2_blocker();
            ui.horizontal_wrapped(|ui| {
                if widgets::ibutton(ui, icon::ARROWS_CLOCKWISE, "Refit the newest run", !busy).clicked() {
                    app.core.eyecal_refit(false);
                }
                let r = widgets::ibutton(ui, icon::CHECK_CIRCLE, "Save the refit...", !busy && blocker.is_none());
                let r = match &blocker {
                    Some(b) => r.on_disabled_hover_text(b.as_str()),
                    None => r,
                };
                if r.clicked() {
                    app.core.eyecal_refit(true);
                }
            });
            if let Some(b) = blocker {
                ui.label(RichText::new(b).size(12.0).color(theme::pal(ui).warn));
            }
        }
        let eye = &live.log.eye;
        if let (true, Some(s)) = (eye.enabled, eye.stats) {
            note(ui, &format!("Now: {:.0} % valid of {} gaze queries, chip {:.0} samples/s", s.valid_pct(), s.queries, s.chip_hz));
        }
    });
    }

    let screens = wayvr::screen_count(app);
    let dw = format!("{} | captions {} | guide", screens.map(|n| format!("{n} screens")).unwrap_or_else(|| "screens".into()), onoff(t.voice));
    row(ui, Id::new("row-desktop"), icon::DESKTOP, "Desktop / WayVR", &dw, |ui| {
        toggle(ui, &mut app.core.toggles.passthrough, "Passthrough (camera view behind the screens)", ed && (caps.no_passthrough || !t.passthrough));
        ui.horizontal(|ui| {
            ui.label(RichText::new("Preset").color(theme::pal(ui).dim));
            ui.add_enabled_ui(ed && caps.preset, |ui| {
                for (pr, ic, label) in [
                    (Preset::None, icon::SQUARES_FOUR, "None"),
                    (Preset::Cinema, icon::POPCORN, "Cinema"),
                    (Preset::Work, icon::BRIEFCASE, "Work"),
                    (Preset::Assistant, icon::ROBOT, "Assistant"),
                ] {
                    let on = app.core.toggles.preset == pr;
                    let r = ui.selectable_label(on, format!("{ic} {label}"));
                    r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, on, label));
                    if r.clicked() {
                        app.core.toggles.preset = pr;
                    }
                }
            });
        });
        note(ui, match app.core.toggles.preset {
            Preset::None => "No preset: the screens as you placed them.",
            Preset::Cinema => "Cinema: curved screens, passthrough dimmed (desk start --preset cinema).",
            Preset::Work => "Work: flat, sharper screens (desk start --preset work).",
            Preset::Assistant => "Assistant: voice captions on (desk start --preset assistant).",
        });
        if dev {
            crate::pages::screens::body(ui, app, ed, caps.screens);
            wayvr::body(ui, app);
        }
    });

    let rm = room::summary(app);
    if std::mem::take(&mut app.core.room_requested) {
        // the onboarding's room step: open the Room row and scroll to it
        widgets::set_row_open(ui.ctx(), room_row(), true);
        app.ui.scroll_room = true;
    }
    let scroll_room = std::mem::take(&mut app.ui.scroll_room);
    let r = ui.scope(|ui| {
        row(ui, room_row(), icon::MAP_TRIFOLD, "Room", &rm, |ui| room::body(ui, app));
    });
    if scroll_room {
        r.response.scroll_to_me(Some(Align::TOP));
    }

    if dev {
    row(ui, Id::new("row-voice"), icon::MICROPHONE, "Voice", &format!("captions {}", onoff(t.voice)), |ui| {
        toggle(ui, &mut app.core.toggles.voice, "Live captions of what you say (desk --voice)", ed && (caps.voice || t.voice));
        note(ui, "A panel under the main screen; nothing is typed. The headset microphone sends zeros while the display is off (LESSONS row 19).");
        wayvr::captions_screen(ui, app);
        ui.horizontal_wrapped(|ui| {
            tool_button(ui, app, ToolId::VoiceTest, "Voice test");
            if widgets::ibutton(ui, icon::ROBOT, "AI settings", true).on_hover_text("The AI page: providers for voice commands and Autofix").clicked() {
                app.core.page = Page::Ai;
            }
            if widgets::ibutton(ui, icon::ARROW_SQUARE_OUT, "Voice commands (research)", true).clicked() {
                let f = app.core.paths.repo.join("analysis/research/omarchy-voice-commands.html");
                app.core.open_folder(&f);
            }
        });
    });
    }

    // HIDDEN (not ready): video recording is "coming soon" and does nothing yet
    // row(ui, Id::new("row-video"), icon::VIDEO_CAMERA, "Video recording", "coming soon", |ui| {
    //     ui.add_enabled_ui(false, |ui| {
    //         ui.horizontal(|ui| {
    //             ui.label("Resolution");
    //             let _ = ui.radio(true, "Auto");
    //             let _ = ui.radio(false, "1080p");
    //             ui.label("Rate");
    //             let _ = ui.radio(true, "60 fps");
    //         });
    //         let mut r = false;
    //         toggle(ui, &mut r, "Instant replay", false);
    //     });
    //     soon(ui);
    // });

    if dev {
    app.core.want_auto();
    let au = match &app.core.auto.status {
        Some(Ok(s)) => format!("{} | {}{}", s.state, if s.dry_run { "dry run" } else { "live" }, if s.paused { " | paused" } else { "" }),
        Some(Err(e)) if e == "not running" => "not running".to_string(),
        Some(Err(_)) => "no answer".to_string(),
        None => "-".to_string(),
    };
    row(ui, Id::new("row-auto"), icon::MAGIC_WAND, "Auto mode", &au, |ui| auto_body(ui, app));
    }

    if dev {
    let rt = if app.core.facts.openvr.xrizer { "Monado + xrizer" } else if app.core.facts.openvr.exists { "SteamVR" } else { "no OpenVR runtime" };
    row(ui, Id::new("row-runtime"), icon::STEAM_LOGO, "VR mode / Runtime", rt, |ui| {
        let o = app.core.facts.openvr;
        note(ui, match (o.exists, o.xrizer, o.read_only) {
            (false, _, _) => "No OpenVR runtime file: Steam games cannot find a runtime.",
            (true, true, true) => "Steam's OpenVR games go to Monado through xrizer (the runtime file is read-only).",
            (true, true, false) => "xrizer + Monado; the runtime file is writable, so a SteamVR install may rewrite it (LESSONS row 20).",
            (true, false, _) => "SteamVR is the OpenVR runtime: Steam games will not reach Monado.",
        });
        ui.horizontal_wrapped(|ui| {
            let busy = app.core.busy();
            if ui.add_enabled(!busy, egui::Button::new("Use xrizer + Monado")).clicked() {
                app.core.runtime("xrizer");
            }
            if ui.add_enabled(!busy, egui::Button::new("Restore SteamVR")).clicked() {
                app.core.runtime("steamvr");
            }
            if ui.add_enabled(!busy, egui::Button::new("Show status")).clicked() {
                app.core.runtime("status");
            }
        });
        caption(ui, "Steam launch options");
        widgets::code_box(ui, &[app.core.steam_options()]);
        ui.horizontal_wrapped(|ui| {
            chip(ui, "Proton games", theme::pal(ui).warn);
            widgets::row_tag(ui, Some(27));
        });
        note(ui, "Every Steam VR game needs the line above in its Launch Options (Steam > game > Properties > General), once: without it Steam's container does not see Monado and the game runs flat. Then start VR here and press Play in Steam; SteamVR mode (no -openxr) is the default.");
    });
    }

    if dev {
    let crashes = app.core.crash_count();
    let dv = format!("{} | log | report", if crashes == 0 { "no crash reports".to_string() } else { format!("{crashes} crash reports") });
    row(ui, Id::new("row-developer"), icon::CODE, "Developer", &dv, |ui| {
        ui.horizontal_wrapped(|ui| {
            if widgets::ibutton(ui, icon::FILE_TEXT, "Logs", true).clicked() {
                app.core.page = Page::Logs;
            }
            if widgets::ibutton(ui, icon::WRENCH, "Setup checklist", true).clicked() {
                app.core.page = Page::Setup;
            }
            if widgets::ibutton(ui, icon::FOLDER_OPEN, "Crash reports", true).clicked() {
                let d = pimax_gui_core::crash::crashes_dir(&app.core.paths.state_dir);
                app.core.open_folder(&d);
            }
            if widgets::ibutton(ui, icon::BUG, "Report a problem...", true).clicked() {
                app.ui.report_open = true;
            }
        });
        note(ui, &format!(
            "Pimax {}. App log: {}",
            pimax_gui_core::crash::version_line(),
            pimax_gui_core::applog::log_file(&app.core.paths.state_dir).display()
        ));
        egui::CollapsingHeader::new("What Start runs").id_salt("plan-dev").show(ui, |ui| desk::plan_body(ui, app));
    });
    }

    // the switch that shows the advanced rows and pages (saved in gui-state.toml)
    ui.add_space(14.0);
    let mut dv = dev;
    if toggle(ui, &mut dv, "Developer mode", true)
        .on_hover_text("Shows the advanced rows and pages: eye tracking, controller and hand tests, screens, voice, auto mode, logs, tools.")
        .changed()
    {
        app.core.set_dev_mode(dv);
    }

    if app.core.toggles != before {
        app.ui.pf = None;
        app.core.save_state();
    }
}

fn right(ui: &mut Ui, app: &mut PimaxApp) {
    let p = theme::pal(ui);
    let devs = devices(&app.core.facts, &app.core.live, &app.core.toggles, app.core.demo);
    ui.horizontal(|ui| {
        // normal mode: a simple choice of what games run with (only options that exist on this PC)
        // the saved choice (~/.config/pimax/games.toml); Play starts Monado or SteamVR (with the Pimax driver) from it
        {
            let (installed, chosen) = (app.core.facts.steamvr_installed, app.core.facts.games_steamvr);
            let steamvr_ok = installed || chosen;
            let sel = if chosen { 1 } else { 0 };
            let free = !app.core.busy() && app.core.phase() == DeskPhase::Idle;
            let names: Vec<&str> = if steamvr_ok { vec!["Monado (xrizer)", "SteamVR (experimental)"] } else { vec!["Monado (xrizer)"] };
            let r = widgets::selector(ui, icon::ARROWS_CLOCKWISE, "Games run with", &names, sel.min(names.len() - 1), free);
            if let Some(i) = r {
                app.core.runtime(if i == 0 { "xrizer" } else { "steamvr" });
            }
            if chosen {
                ui.label(RichText::new("Play starts SteamVR with the Pimax driver").size(11.5).color(theme::pal(ui).faint))
                    .on_hover_text("SteamVR drives the headset itself (no Monado). Experimental: see Help > SteamVR. Stop Play closes SteamVR.");
            }
        }
        {
            let mut on = app.core.facts.sound_headset;
            let free = !app.core.busy();
            let r = widgets::toggle(ui, &mut on, "Sound in headset", free);
            let r = r.on_hover_text("While VR runs (Play, Work, SteamVR) the sound goes to the headset's speakers: they get it over the video cable (DisplayPort audio), a few seconds after the headset lights up. Stop puts the old output back. Sunshine is left alone.");
            if r.changed() {
                app.core.sound_headset(if on { "on" } else { "off" });
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            pill(ui, Id::new("tracking-badge"), &format!("{} {}", icon::CROSSHAIR, devs.tracking.0), widgets::dev_color(&p, devs.tracking.1));
        });
    });
    // the headset speaker test: a short chime on the headset speaker only (docs/AUDIO.md)
    ui.horizontal_wrapped(|ui| {
        let free = !app.core.busy();
        if widgets::ghost(ui, icon::HEADSET, "Test headset sound", free)
            .on_hover_text("Plays two soft notes on the headset speaker only. The speaker exists while VR runs (Play, Work or SteamVR), a few seconds after the headset lights up.")
            .clicked()
        {
            app.core.speaker_test();
        }
        if let Some(r) = &app.core.speaker_result {
            let good = r.starts_with("played");
            ui.label(RichText::new(r).size(11.5).color(if good { p.ok } else { p.warn }));
        }
    });
    // the microphone: switch, what the default input is now, and a 3 s test (docs/AUDIO.md)
    ui.horizontal_wrapped(|ui| {
        let mut on = app.core.facts.mic_headset;
        let free = !app.core.busy();
        let r = widgets::toggle(ui, &mut on, "Headset microphone", free).on_hover_text(
            "While VR runs the headset microphone is the default input (voice commands, games, calls); Stop puts the old input back. Voice captions always listen to the headset microphone.",
        );
        if r.changed() {
            app.core.sound_headset(if on { "mic-on" } else { "mic-off" });
        }
        let kind = app.core.facts.mic_kind();
        let txt = match kind {
            Some("headset") => "Microphone: headset",
            Some(_) => "Microphone: other",
            None => "Microphone: unknown",
        };
        ui.label(RichText::new(txt).size(11.5).color(p.dim))
            .on_hover_text(app.core.facts.mic_default.clone().unwrap_or_else(|| "pactl did not answer".into()));
        if widgets::ghost(ui, icon::MICROPHONE, "Test microphone", free)
            .on_hover_text("Records 3 seconds from the headset microphone, shows the level and plays it back on the current output. The headset microphone sends zeros while the headset display is off: start Play or Work first.")
            .clicked()
        {
            app.core.mic_test();
        }
        if let Some(r) = &app.core.mic_result {
            let good = r.ends_with(": good");
            ui.label(RichText::new(r).size(11.5).color(if good { p.ok } else { p.warn }));
        }
    });
    ui.add_space(12.0);
    headset_card(ui, app, &devs.headset);
    ui.add_space(10.0);
    ui.columns(2, |c| {
        for (i, (id, v)) in [("dev-left", &devs.left), ("dev-right", &devs.right)].into_iter().enumerate() {
            let side = if i == 0 { "left" } else { "right" };
            device_card(&mut c[i], Id::new(id), icon::GAME_CONTROLLER, v, false, |ui| pair_ui(ui, app, side));
        }
    });
    ui.add_space(14.0);
    session(ui, app);
    ui.add_space(12.0);
    super::record::card_ui(ui, app);
    ui.add_space(12.0);
    ui.vertical_centered(|ui| {
        if widgets::ghost(ui, icon::HOUSE, "Room setup", true).clicked() {
            widgets::set_row_open(ui.ctx(), room_row(), true);
            app.ui.scroll_room = true;
        }
    });
    ui.add_space(12.0);
    if app.core.state.dev_mode {
        egui::CollapsingHeader::new(RichText::new("Session details").color(p.dim)).id_salt("session-details").show(ui, |ui| desk::status(ui, app));
    }
}

fn headset_card(ui: &mut Ui, app: &mut PimaxApp, v: &DeviceView) {
    let detected = headsets::connected(&app.core.facts).map(|m| m.id().to_string());
    let chosen: String = app.ui.model.map(String::from).or(detected).unwrap_or_else(|| headsets::default_module().id().to_string());
    let model = headsets::by_id(&chosen).unwrap_or_else(headsets::default_module);
    let mut pick: Option<&'static str> = None;
    let picker = |ui: &mut Ui, pick: &mut Option<&'static str>| {
        egui::ComboBox::from_id_salt("headset-model")
            .selected_text(RichText::new("Change headset").size(11.5))
            .width(150.0)
            .show_ui(ui, |ui| {
                for m in headsets::all().iter() {
                    let id: &'static str = m.id();
                    let name = &m.descriptor().name;
                    let label = if m.is_mock() { format!("{name} (not supported yet)") } else { name.clone() };
                    if ui.selectable_label(id == chosen, label).clicked() {
                        *pick = Some(id);
                    }
                }
            });
    };
    if !model.is_mock() {
        device_card(ui, Id::new("dev-headset"), icon::GOGGLES, v, true, |ui| {
            ui.horizontal(|ui| {
                picker(ui, &mut pick);
            });
        });
        reset_state(ui, app);
    } else {
        let d = model.descriptor();
        let mock = DeviceView {
            name: d.name.clone(),
            state: Dev::Off,
            label: "Not supported yet".into(),
            detail: "Listed so the app has room for it; nothing is known to work yet".into(),
            battery: None,
        };
        device_card(ui, Id::new("dev-headset"), icon::GOGGLES, &mock, true, |ui| {
            picker(ui, &mut pick);
            ui.add_space(4.0);
            for c in headsets::caps(d) {
                ui.label(RichText::new(format!("{}  - {}", c.name, headsets::support_label(c.support, true))).size(11.5).color(theme::pal(ui).faint));
            }
        });
    }
    if let Some(id) = pick {
        app.ui.model = Some(id);
    }
}

/// A running headset restart (plain stage and seconds against its 30) or the last one's result.
pub(crate) fn reset_state(ui: &mut Ui, app: &mut PimaxApp) {
    let p = theme::pal(ui);
    if let Some(k) = app.core.restart_headset_progress() {
        let total = app.core.restart_headset_total_secs();
        let secs = (k * total as f32) as u64;
        let stage = app.core.restart_headset_stage().unwrap_or_default();
        ui.add(egui::ProgressBar::new(k).desired_width(ui.available_width()).text(format!("{stage} ({secs} s of about {total} s)")));
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    } else if let Some(r) = &app.core.headset_reset {
        ui.label(RichText::new(&r.text).size(12.0).color(if r.ok { p.ok } else { p.warn }));
    }
    if let Some(k) = app.core.restart_service_progress() {
        let secs = (k * pimax_gui_core::reset::RESTART_SECS as f32) as u64;
        ui.add(egui::ProgressBar::new(k).desired_width(ui.available_width()).text(format!("Restarting the service: {secs} s")));
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    }
    if let Some(w) = app.core.watchdog_state() {
        ui.add(egui::Label::new(RichText::new(format!("{} {w}", icon::PULSE)).size(11.5).color(p.dim)).truncate());
    }
}

/// The readiness line: headset, the Monado cap (with Fix), the OpenXR runtime, the last start.
fn readiness(ui: &mut Ui, app: &mut PimaxApp) {
    let p = theme::pal(ui);
    let r = app.core.readiness();
    let color = |ok: Option<bool>| match ok {
        Some(true) => p.ok,
        Some(false) => p.warn,
        None => p.faint,
    };
    let item = |ui: &mut Ui, ok: Option<bool>, text: &str| {
        widgets::dot(ui, color(ok), 4.0);
        ui.label(RichText::new(text).size(12.0).color(p.dim));
        ui.add_space(6.0);
    };
    let mut fix = false;
    ui.horizontal_wrapped(|ui| {
        item(ui, r.headset.0, &r.headset.1);
        match &r.cap {
            CapState::Ok(s) => {
                widgets::dot(ui, p.ok, 4.0);
                ui.label(RichText::new("Monado cap OK").size(12.0).color(p.dim)).on_hover_text(format!("{} has cap_sys_nice", s.display()));
                ui.add_space(6.0);
            }
            CapState::Missing(s) => {
                widgets::dot(ui, p.fail, 4.0);
                ui.label(RichText::new("Monado cap missing").size(12.0).color(p.fail))
                    .on_hover_text(format!("{} lost cap_sys_nice (every rebuild drops it): frames can stutter. Fix asks for your password once.", s.display()));
                if widgets::ibutton(ui, icon::WRENCH, "Fix", !app.core.busy())
                    .on_hover_text("pkexec setcap cap_sys_nice+ep on this monado-service (a password window), then checks again")
                    .clicked()
                {
                    fix = true;
                }
                ui.add_space(6.0);
            }
            CapState::Unknown(_) => item(ui, None, "Monado cap: checking"),
        }
        item(ui, r.runtime.0, &r.runtime.1);
        item(ui, r.last.0, &r.last.1);
    });
    if let Some(m) = &app.core.play.last_msg {
        ui.add(egui::Label::new(RichText::new(m).size(11.5).color(p.faint)).truncate());
    }
    if fix {
        app.core.fix_cap();
    }
}

/// The guided hand test (in the headset) and its last result.
fn hand_test(ui: &mut Ui, app: &mut PimaxApp) {
    let p = theme::pal(ui);
    caption(ui, "Guided hand test");
    note(ui, "Restarts desk mode with 6DoF and camera hands, then shows 4 steps of 10 s in the headset (left hand, right hand, both, open/close and pinch) while it records the cameras into ~/src/cam-data. Then it measures the detection rate.");
    let blockers = app.core.hand_test_blockers();
    let r = widgets::ibutton(ui, icon::HAND, "Start the hand test", blockers.is_empty());
    let r = if blockers.is_empty() { r } else { r.on_disabled_hover_text(blockers.join("; ")) };
    if r.clicked() {
        app.core.start_hand_test();
    }
    if !blockers.is_empty() && !app.core.busy() {
        ui.label(RichText::new(format!("Not now: {}", blockers.join("; "))).size(11.5).color(p.faint));
    }
    if let Some(res) = app.core.play.hand_test.result.clone() {
        let (text, c) = match &res.detection {
            Some(d) => (format!("Detection: {d}"), p.ok),
            None => ("No detection rate".to_string(), p.warn),
        };
        ui.label(RichText::new(text).size(12.5).color(c));
        for l in &res.live {
            ui.add(egui::Label::new(RichText::new(l).size(11.0).monospace().color(p.dim)).truncate());
        }
        if let Some(n) = &res.note {
            ui.label(RichText::new(n).size(11.5).color(p.warn));
        }
        if let Some(f) = &app.core.play.hand_test.file {
            ui.label(RichText::new(format!("Recording: {}", f.display())).size(11.0).color(p.faint));
        }
    }
}

/// The start/stop area: the readiness line, the two big buttons Play (games) and Work (the desk; the button of the
/// running side stops it), the verdict of the preflight, its details, start at login.
/// "Restart VR service" (stop with a 30 s grace, start again with the same switches) and "Restart headset" (the
/// headset's own power cycle, `pimax vr reset-headset`). Each is disabled with its reason while another job runs
/// or while it cannot run (a session must be stopped before the headset restarts).
fn restart_buttons(ui: &mut Ui, app: &mut PimaxApp) {
    let svc = app.core.restart_service_blocker();
    let hd = app.core.restart_headset_blocker();
    let half = (ui.available_width() - 10.0) / 2.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let r = ui.add_enabled_ui(svc.is_none(), |ui| {
            ui.add_sized([half, 30.0], egui::Button::new(format!("{} Restart VR service", icon::POWER)))
        });
        let r = match &svc {
            Some(b) => r.inner.on_disabled_hover_text(b.as_str()),
            None => r.inner.on_hover_text("Stops the running session (30 s grace) and starts it again with the same switches. Clients are stopped by process id."),
        };
        if r.clicked() {
            app.core.restart_service();
        }
        let r = ui.add_enabled_ui(hd.is_none(), |ui| {
            ui.add_sized([half, 30.0], egui::Button::new(format!("{} Restart headset", icon::ARROWS_CLOCKWISE)))
        });
        let r = match &hd {
            Some(b) => r.inner.on_disabled_hover_text(b.as_str()),
            None => r.inner.on_hover_text("Sends the headset its own restart command (what Pimax Play's restart button sends): it is back on USB after about 8 s and shows a picture after about 22 s. Not while VR runs. Keep it still."),
        };
        if r.clicked() {
            app.core.restart_headset();
        }
    });
}

fn session(ui: &mut Ui, app: &mut PimaxApp) {
    let p = theme::pal(ui);
    readiness(ui, app);
    ui.add_space(6.0);
    let phase = app.core.phase();
    let busy = app.core.busy();
    let w = ui.available_width();
    // Games run with: SteamVR: Steam restarted or closed, a Steam that SteamVR started, SteamVR quit (docs/STEAMVR.md)
    if let Some(n) = &app.core.live.steam_note {
        ui.label(RichText::new(n).color(p.warn));
    }
    match phase {
        DeskPhase::Running => {
            let age = app.core.live.service.as_ref().and_then(|s| s.age).map(fmt_age).unwrap_or_default();
            let vr = app.core.running_side() == Some(true);
            let mode = if vr { "Play (game session)" } else { "Work (desk)" };
            ui.label(RichText::new(format!("{mode} running {age}")).color(p.ok));
            if let Some(sp) = app.core.live.scale_pct {
                ui.label(RichText::new(format!("Render: {}", pimax_gui_core::quality::describe(sp))).size(11.5).color(p.dim));
            }
            if !app.core.live.gm_paused.is_empty() {
                ui.label(RichText::new(format!("Paused for game mode: {}", app.core.live.gm_paused.join(", "))).size(11.5).color(p.dim))
                    .on_hover_text("Stop Play starts them again.");
            }
            if let Some(ps) = &app.core.live.perf {
                let c = if ps.dropped_pct() > 1.0 { p.warn } else { p.dim };
                ui.label(RichText::new(ps.summary()).size(11.5).color(c)).on_hover_text(format!(
                    "Dropped = the headset showed an image twice ({} of {} frames since Monado started). Separately, the compositor's \
                     own commit (render, present, wait for the GPU) ended after the predicted display time in {:.0} % of frames: \
                     not a dropped frame, but a sign of compositor latency (Help > Render quality).",
                    ps.skipped, ps.frames, ps.late_pct()
                ));
            }
            let half = (w - 10.0) / 2.0;
            let hover = "pimax vr stop / desk stop --grace 30: clients by pid, then Monado (SIGINT; SIGKILL only after 30 s)";
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                for (side_vr, label) in [(true, "Stop Play"), (false, "Stop Work")] {
                    if side_vr == vr {
                        if widgets::action(ui, icon::STOP, label, Kind::Danger, !busy, half).on_hover_text(hover).clicked() {
                            app.core.stop();
                        }
                    } else {
                        let other = if side_vr { "Play" } else { "Work" };
                        let _ = widgets::action(ui, if side_vr { icon::PLAY } else { icon::DESKTOP }, other, Kind::Secondary, false, half)
                            .on_disabled_hover_text("Stop the running session first");
                    }
                }
            });
        }
        DeskPhase::Foreign => {
            ui.label(RichText::new("Another monado-service is running").color(p.fail));
            if widgets::action(ui, icon::STOP, "Stop other Monado", Kind::Danger, !busy, w).clicked() {
                app.core.stop_foreign();
            }
        }
        DeskPhase::Idle | DeskPhase::Stopping if !busy => {
            let vpf = app.core.vr_preflight();
            let pf = app.preflight();
            let can = |pf: &Preflight, consent: bool| match &pf.gate {
                Gate::Ready => true,
                Gate::NeedsConsent(_) => consent,
                Gate::Blocked(_) => false,
            };
            let consent = app.core.consent_force;
            let cap = app.core.cap_missing().is_some() && app.core.facts.polkit_agent;
            let first = if cap { " First restores cap_sys_nice on monado-service (one password window)." } else { "" };
            let half = (w - 10.0) / 2.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                if widgets::action(ui, icon::PLAY, "Play", Kind::Primary, can(&vpf, consent), half)
                    .on_hover_text(format!("Games, like SteamVR: the Pimax services (Monado, tracking, controllers, hands; pimax vr start) for Steam and OpenXR games. No desk.{first}"))
                    .clicked()
                {
                    app.core.play();
                }
                if widgets::action(ui, icon::DESKTOP, "Work", Kind::Secondary, can(&pf, consent), half)
                    .on_hover_text(format!("The WayVR desk on top: screens, passthrough, voice, AI, captions, presets (pimax desk start with the switches on the left).{first}"))
                    .clicked()
                {
                    app.core.work();
                }
            });
            if cap {
                ui.label(RichText::new("Play and Work first restore the Monado cap: your password is asked once.").size(11.5).color(p.warn));
            }
            let last = if app.core.play_is_vr() { vpf } else { pf };
            verdict(ui, app, &last);
        }
        _ => {
            ui.horizontal(|ui| {
                widgets::spinner(ui);
                ui.label(RichText::new(app.core.job.as_ref().map(|j| j.label.clone()).unwrap_or_else(|| "Working...".into())).color(p.dim));
            });
            let _ = widgets::action(ui, icon::PLAY, "Working...", Kind::Secondary, false, w);
        }
    }
    restart_buttons(ui, app);
    // subtitles: live while a desk session runs, else the default of the next start
    ui.add_space(6.0);
    ui.horizontal(|ui| match app.core.subtitles_live() {
        Some(on) => {
            let mut v = on;
            if toggle(ui, &mut v, "Subtitles", !busy).on_hover_text("Live subtitles of what you say, under the main screen (pimax desk captions on/off)").changed() {
                app.core.set_subtitles(v);
            }
        }
        None => {
            let mut d = app.core.toggles.voice;
            if toggle(ui, &mut d, "Subtitles at desk start", !busy).on_hover_text("Adds --voice to Start Desk").changed() {
                app.core.set_subtitles_default(d);
            }
        }
    });
    let mut auto = app.core.state.autostart;
    if toggle(ui, &mut auto, "Start VR when I log in", true)
        .on_hover_text("When the app opens at login, the last side you used (Play or Work) starts by itself if the headset is connected, the Monado cap is set and the checks are clear. Never asks for a password.")
        .changed()
    {
        app.core.set_autostart(auto);
    }
    match &app.core.play.autostart_note {
        Some((ok, n)) => {
            ui.label(RichText::new(n).size(11.5).color(if *ok { p.ok } else { p.warn }));
        }
        None if app.core.play.autostart_pending => {
            ui.label(RichText::new("Start at login: waiting for the first check and the headset...").size(11.5).color(p.faint));
        }
        None => {}
    }
}

/// One line for the preflight, the consent box, the details.
fn verdict(ui: &mut Ui, app: &mut PimaxApp, pf: &Preflight) {
    let p = theme::pal(ui);
    ui.add_space(6.0);
    let (text, color) = match &pf.gate {
        Gate::Ready => ("Ready to start".to_string(), p.ok),
        Gate::NeedsConsent(r) => (format!("Needs your OK: {}", r.join("; ")), p.warn),
        Gate::Blocked(r) => (format!("Blocked: {}", r.join("; ")), p.fail),
    };
    ui.horizontal_wrapped(|ui| {
        pill(ui, Id::new("verdict"), match &pf.gate {
            Gate::Ready => "ready",
            Gate::NeedsConsent(_) => "your OK",
            Gate::Blocked(_) => "blocked",
        }, color);
        ui.add(egui::Label::new(RichText::new(text).size(12.5).color(p.dim)).wrap());
    });
    if matches!(pf.gate, Gate::NeedsConsent(_)) {
        ui.checkbox(&mut app.core.consent_force, RichText::new("Start anyway: the only problem is WayVR Xid 31 faults (adds --force)").color(p.warn))
            .on_hover_text("LESSONS row 9: per-app MMU faults the GPU recovers from. Any other Xid or a display engine error blocks the start.");
    }
    egui::CollapsingHeader::new(RichText::new(format!("Preflight ({} checks)", pf.findings.len())).color(p.dim))
        .id_salt("preflight-details")
        .show(ui, |ui| inset(ui, |ui| desk::banner_body(ui, app, pf)));
}

/// The Auto mode row: the daemon's state, dry run or live, pause, mode, runtime, preset, start and sleep now.
fn auto_body(ui: &mut Ui, app: &mut PimaxApp) {
    let p = theme::pal(ui);
    match app.core.auto.status.clone() {
        None => {
            ui.horizontal(|ui| {
                widgets::spinner(ui);
                ui.label(RichText::new("Asking the daemon...").color(p.dim));
            });
        }
        Some(Err(e)) => {
            note(ui, if e == "not running" {
                "Auto mode (pimax-autod) is not running. It starts the session when you put the headset on and lets it sleep when you leave; dry run until dry_run = false in ~/.config/pimax/auto.toml."
            } else {
                "The auto mode daemon did not answer."
            });
            if e != "not running" {
                ui.label(RichText::new(&e).color(p.warn));
            }
            let busy = app.core.busy();
            if app.core.facts.manual.autod_unit.is_some() && app.core.facts.manual.autod_build {
                ui.horizontal_wrapped(|ui| {
                    if widgets::ibutton(ui, icon::PLAY, "Start auto mode", !busy).on_hover_text("systemctl --user start pimax-autod (dry run until auto.toml says otherwise)").clicked() {
                        app.core.autod_unit("start");
                    }
                    if widgets::ibutton(ui, icon::POWER, "Start at login", !busy).on_hover_text("systemctl --user enable pimax-autod").clicked() {
                        app.core.autod_unit("enable");
                    }
                });
            } else {
                note(ui, "Not installed yet: Setup > Install > Auto mode daemon builds it and installs its user unit and a dry-run config (nothing starts by itself).");
                if widgets::ibutton(ui, icon::WRENCH, "Install it", true).clicked() {
                    app.core.act(&pimax_gui_core::preflight::Action::Setup("auto-mode".into()));
                }
            }
        }
        Some(Ok(s)) => {
            ui.horizontal_wrapped(|ui| {
                let c = if s.state.starts_with("active") { p.ok } else if s.state.starts_with("idle") { p.warn } else { p.accent };
                pill(ui, Id::new("auto-state"), &s.state, c);
                if s.dry_run {
                    chip(ui, "dry run", p.warn);
                } else {
                    chip(ui, "live", p.ok);
                }
                if s.paused {
                    chip(ui, "paused", p.faint);
                }
                if let Some(b) = &s.busy {
                    chip(ui, b, p.info);
                }
            });
            note(ui, &format!(
                "Headset {}, {}{}{}; up {}",
                if s.plugged { "plugged in" } else { "unplugged" },
                if s.worn { "worn" } else { "not worn" },
                s.session.as_ref().map(|x| format!(", {x} session")).unwrap_or_default(),
                if s.game { ", a game runs" } else { "" },
                fmt_age(s.uptime_s as f64)
            ));
            let mut active = !s.paused;
            if toggle(ui, &mut active, "Auto mode acts (off = paused: states are still tracked)", true).changed() {
                app.core.auto_cmd(if active { AutoCmd::Resume } else { AutoCmd::Pause });
            }
            caption(ui, "Session to start");
            let mode = if s.mode == "game" { 1 } else { 0 };
            if let Some(i) = widgets::selector(ui, icon::GOGGLES, "Mode", &["Desk", "Game"], mode, true) {
                app.core.auto_cmd(AutoCmd::SetMode { mode: Some(if i == 1 { "game" } else { "desk" }.into()), runtime: None, preset: None });
            }
            let rt = if s.game_runtime == "steamvr" { 1 } else { 0 };
            if let Some(i) = widgets::selector(ui, icon::STEAM_LOGO, "Game runtime", &["Monado + xrizer", "SteamVR (start it yourself)"], rt, true) {
                app.core.auto_cmd(AutoCmd::SetMode { mode: None, runtime: Some(if i == 1 { "steamvr" } else { "monado" }.into()), preset: None });
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new("Preset").color(p.dim));
                for pr in Preset::ALL {
                    let on = s.preset == pr.name();
                    let label = match pr {
                        Preset::None => "None",
                        Preset::Cinema => "Cinema",
                        Preset::Work => "Work",
                        Preset::Assistant => "Assistant",
                    };
                    let r = ui.selectable_label(on, label);
                    if r.clicked() && !on {
                        app.core.auto_cmd(AutoCmd::SetMode { mode: None, runtime: None, preset: Some(pr.name().into()) });
                    }
                }
            });
            note(ui, "Changes here last until the daemon restarts; ~/.config/pimax/auto.toml keeps them for good.");
            ui.horizontal_wrapped(|ui| {
                if widgets::ibutton(ui, icon::PLAY, "Start now", !s.paused).on_hover_text("wake: counts as wearing the headset").clicked() {
                    app.core.auto_cmd(AutoCmd::Wake);
                }
                if widgets::ibutton(ui, icon::POWER, "Sleep now", s.owned).on_hover_text("sleep: stops the session auto mode started (never a game)").clicked() {
                    app.core.auto_cmd(AutoCmd::Sleep);
                }
            });
            caption(ui, "Controllers");
            ui.horizontal(|ui| {
                pair_ui(ui, app, "left");
                pair_ui(ui, app, "right");
            });
        }
    }
    if !app.core.auto.last.is_empty() {
        ui.label(RichText::new(&app.core.auto.last).size(12.0).color(p.faint));
    }
}

/// Pair one controller: the button, then the instruction with a 35 s progress, then the result. At most one try per
/// hand every 30 s (the button says when the next one is possible).
pub(crate) fn pair_ui(ui: &mut Ui, app: &mut PimaxApp, side: &'static str) {
    pair_buttons(ui, app, side);
    unpair_ui(ui, app, side);
    buzz_ui(ui, app, side);
}

/// "Test vibration": Pimax Play's test pulse (25 %, 300 ms, `58 09 07 SS 19 2c 01`); through the running VR session
/// when there is one. The result shows on the message line.
fn buzz_ui(ui: &mut Ui, app: &mut PimaxApp, side: &'static str) {
    let busy = app.core.task_busy("buzz");
    if widgets::ibutton(ui, icon::PULSE, "Test vibration", !busy)
        .on_hover_text("A short buzz (25 %, 300 ms), like Pimax Play's test. Works with or without VR running; the controller must be awake.")
        .clicked()
    {
        app.core.buzz(side);
    }
}

/// Unpair one controller (`58 03 05 SS`): the button, then a one-line confirm; the result shows on the message line.
fn unpair_ui(ui: &mut Ui, app: &mut PimaxApp, side: &'static str) {
    use pimax_gui_core::auto::PairState;
    let p = theme::pal(ui);
    let id = Id::new(("unpair-confirm", side));
    let asking = ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let pairing = app.core.auto.pair.as_ref().is_some_and(|x| x.2 == PairState::Waiting);
    // who owns the headset (cached 2 s: it scans /proc)
    let oid = Id::new("vr-owner");
    let owner: Option<&'static str> = match ui.ctx().data(|d| d.get_temp::<(std::time::Instant, Option<&'static str>)>(oid)) {
        Some((t, o)) if t.elapsed().as_secs() < 2 => o,
        _ => {
            let o = if app.core.demo { None } else { pimax_gui_core::auto::vr_session_owner() };
            ui.ctx().data_mut(|d| d.insert_temp(oid, (std::time::Instant::now(), o)));
            o
        }
    };
    let why = if let Some(w) = owner {
        format!("{w} is running and owns the headset: stop VR first, then unpair")
    } else {
        "Not while a pairing runs".to_string()
    };
    let can = !pairing && !app.core.busy() && owner.is_none();
    if asking && can {
        ui.label(RichText::new(format!("Unpair the {side} controller? It must be paired again.")).size(11.5).color(p.warn));
        if widgets::ibutton(ui, icon::LINK_SIMPLE, "Yes, unpair", can).clicked() {
            ui.ctx().data_mut(|d| d.insert_temp(id, false));
            app.core.unpair(side);
        }
        if widgets::ibutton(ui, icon::LINK_SIMPLE, "Cancel", true).clicked() {
            ui.ctx().data_mut(|d| d.insert_temp(id, false));
        }
    } else if widgets::ibutton(ui, icon::LINK_SIMPLE, &format!("Unpair {side}"), can)
        .on_hover_text("Disconnects this controller from the headset. Pair it again afterwards.")
        .on_disabled_hover_text(why.as_str())
        .clicked()
    {
        ui.ctx().data_mut(|d| d.insert_temp(id, true));
    }
    if !can && owner.is_some() {
        ui.label(RichText::new(&why).size(11.0).color(p.faint));
    }
}

fn pair_buttons(ui: &mut Ui, app: &mut PimaxApp, side: &'static str) {
    use pimax_gui_core::auto::{PairState, PAIR_HINT, PAIR_SECS};
    let p = theme::pal(ui);
    let mine = app.core.auto.pair.clone().filter(|x| x.0 == side);
    let other_waiting = app.core.auto.pair.as_ref().is_some_and(|x| x.0 != side && x.2 == PairState::Waiting);
    let wait = app.core.pair_wait(side);
    if wait.is_some() {
        ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
    }
    let can = !other_waiting && !app.core.busy() && wait.is_none();
    let wait_text = wait.map(|s| format!("Again in {s} s (one try per hand every 30 s)")).unwrap_or_default();
    match mine {
        Some((_, t0, PairState::Waiting, _)) => {
            ui.label(RichText::new(PAIR_HINT).size(11.5).color(p.dim));
            let k = (t0.elapsed().as_secs_f32() / PAIR_SECS as f32).min(1.0);
            ui.add(egui::ProgressBar::new(k).desired_width(150.0).text(format!("{:.0} / {PAIR_SECS} s", t0.elapsed().as_secs_f32().min(PAIR_SECS as f32))));
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        }
        Some((_, _, st, detail)) => {
            let (text, c) = match st {
                PairState::Paired => ("Paired", p.ok),
                PairState::Timeout => ("Not paired (timeout)", p.warn),
                _ => ("Not paired", p.fail),
            };
            ui.label(RichText::new(text).size(12.0).color(c)).on_hover_text(detail);
            let r = widgets::ibutton(ui, icon::LINK_SIMPLE, &format!("Pair {side} again"), can);
            if r.on_disabled_hover_text(wait_text.as_str()).clicked() {
                app.core.pair(side);
            }
        }
        None => {
            let label = format!("Pair {side}");
            if widgets::ibutton(ui, icon::LINK_SIMPLE, &label, can)
                .on_hover_text("Put the controller in pairing mode first, then press this and wait up to 60 s (one hand at a time)")
                .on_disabled_hover_text(wait_text.as_str())
                .clicked()
            {
                app.core.pair(side);
            }
        }
    }
}
