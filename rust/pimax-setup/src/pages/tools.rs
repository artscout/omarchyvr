//! Tools: eye view, IPD, eye module power, eye calibration, lens patterns, 6DoF walk test, controller recording,
//! hands test, voice test, Steam runtime switch. Each shows what keeps it from starting (with a button that resolves
//! it) before it can be started.
use crate::app::PimaxApp;
use crate::widgets::{self, card, chip, note, pal, title};
use eframe::egui::{self, RichText, Ui};
use pimax_gui_core::facts::gib;
use pimax_gui_core::preflight::Action;
use pimax_gui_core::monlog::TrackState;
use pimax_gui_core::procs::fmt_age;
use pimax_gui_core::tools::{recording_bytes, walk_script_line, Launch, ToolId, PATTERNS};

enum Click {
    Run(ToolId),
    Act(Action),
    StopSession,
    FinishWalk,
    Reopen,
    Runtime(&'static str),
}

fn action_label(a: &Action) -> String {
    match a {
        Action::StopSession => "Stop desk/VR".into(),
        Action::StopForeignMonado => "Stop other Monado".into(),
        Action::InterruptPid { pid, comm } => format!("SIGINT {comm} ({pid})"),
        Action::Setup(_) => "Open in Setup".into(),
        Action::WithoutEye => "Restart desk without eye tracking".into(),
    }
}

/// Draw the page.
pub fn show(ui: &mut Ui, app: &mut PimaxApp) {
    card(ui, |ui| {
        ui.label(RichText::new("Tools").size(24.0).strong());
        note(ui, "Text tools open in your terminal ($TERMINAL, else foot, alacritty, kitty). Device tools wait for the headset lock. Tools that need their own Monado start it with pimax vr start and stop it with pimax vr stop --grace 30.");
    });
    let mut click: Option<Click> = None;
    let busy = app.core.busy();
    let mut section = "";
    for t in ToolId::ALL {
        if t.section() != section {
            section = t.section();
            ui.add_space(10.0);
            ui.label(RichText::new(section).size(18.0).strong());
        }
        ui.add_space(4.0);
        let avail = app.core.tool_availability(t);
        let session_here = app.core.session_tool.as_ref().is_some_and(|s| s.tool == t);
        card(ui, |ui| {
            ui.horizontal(|ui| {
                title(ui, t.title());
                for r in t.rows() {
                    widgets::row_tag(ui, Some(*r));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if t == ToolId::SteamRuntime {
                        return;
                    }
                    let kind = match pimax_gui_core::tools::launch(t, &app.core.live, &app.core.facts, &app.core.tool_opts, &app.core.paths, "preview") {
                        Launch::Terminal { .. } => "Open in terminal",
                        Launch::Job(_) => "Run",
                        Launch::Session { .. } => "Start session",
                    };
                    let can = avail.ready() && !busy && !session_here;
                    let b = widgets::primary(ui, kind, can);
                    if b.clicked() {
                        click = Some(Click::Run(t));
                    }
                });
            });
            note(ui, t.about());
            options(ui, app, t, avail.needs_consent);
            for b in &avail.blockers {
                ui.horizontal_wrapped(|ui| {
                    chip(ui, "blocked", pal(ui).fail);
                    ui.add(egui::Label::new(RichText::new(&b.text).color(pal(ui).fail)).wrap());
                    if let Some(a) = &b.action {
                        if ui.small_button(action_label(a)).clicked() {
                            click = Some(Click::Act(a.clone()));
                        }
                    }
                });
            }
            for n in &avail.notes {
                ui.horizontal_wrapped(|ui| {
                    chip(ui, "note", pal(ui).warn);
                    ui.add(egui::Label::new(RichText::new(n).color(pal(ui).warn)).wrap());
                });
            }
            if session_here && t == ToolId::WalkTest {
                walk_panel(ui, app, busy, &mut click);
            } else if session_here {
                let has_client = app.core.session_tool.as_ref().is_some_and(|s| s.script.is_some());
                ui.horizontal(|ui| {
                    chip(ui, "session running", pal(ui).ok);
                    if has_client && ui.small_button("Open the client again").clicked() {
                        click = Some(Click::Reopen);
                    }
                    if ui.add_enabled(!busy, egui::Button::new("Stop session (pimax vr stop --grace 30)")).clicked() {
                        click = Some(Click::StopSession);
                    }
                });
            }
            if t == ToolId::WalkTest {
                if let Some(r) = &app.core.walk_result {
                    ui.horizontal_wrapped(|ui| {
                        chip(ui, "last walk", pal(ui).info);
                        ui.add(egui::Label::new(r.as_str()).wrap());
                    });
                }
                egui::CollapsingHeader::new("By hand: analysis/tools/run_slam_view.sh").id_salt("walk-script").show(ui, |ui| {
                    widgets::code_box(ui, &[walk_script_line(&app.core.paths, &app.core.tool_opts)]);
                    note(ui, "Copy it into a terminal yourself when you need its pose printer output. The app does not run it: the script starts Monado with a copied environment and stops it with pkill -x (LESSONS rows 1 and 5); check that nothing runs first.");
                });
            }
            if t == ToolId::SteamRuntime {
                steam(ui, app, busy, &mut click);
            }
        });
    }
    match click {
        Some(Click::Run(t)) => app.core.run_tool(t),
        Some(Click::Act(a)) => app.core.act(&a),
        Some(Click::StopSession) => app.core.stop_tool_session(),
        Some(Click::FinishWalk) => app.core.finish_walk(),
        Some(Click::Reopen) => {
            if let Some(s) = app.core.session_tool.clone() {
                if let Some(script) = &s.script {
                    app.core.open_terminal(&s.title, script);
                }
            }
        }
        Some(Click::Runtime(w)) => app.core.runtime(w),
        None => {}
    }
}

fn consent(ui: &mut Ui, app: &mut PimaxApp, needs: bool) {
    if needs {
        ui.checkbox(
            &mut app.core.tool_opts.consent_force,
            RichText::new("Start anyway over the WayVR Xid 31 faults (--force)").color(pal(ui).warn),
        );
    }
}

fn options(ui: &mut Ui, app: &mut PimaxApp, t: ToolId, needs_consent: bool) {
    let o = &mut app.core.tool_opts;
    match t {
        ToolId::EyeView => {
            ui.checkbox(&mut o.eye_log, "Log the samples to ~/eye-watch-<time>.csv");
        }
        ToolId::EyeCalibration => {
            ui.horizontal(|ui| {
                ui.label("Targets");
                ui.radio_value(&mut o.eyecal_points, 9, "9");
                ui.radio_value(&mut o.eyecal_points, 13, "13 (quadratic fit)");
            });
            consent(ui, app, needs_consent);
        }
        ToolId::LensTest => {
            ui.horizontal(|ui| {
                ui.label("Pattern");
                egui::ComboBox::from_id_salt("pattern")
                    .selected_text(if o.pattern.is_empty() { "guided (all)".to_string() } else { o.pattern.clone() })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut o.pattern, String::new(), "guided (all)");
                        for p in PATTERNS {
                            ui.selectable_value(&mut o.pattern, p.to_string(), p);
                        }
                    });
                ui.radio_value(&mut o.hz90, false, "72 Hz");
                ui.radio_value(&mut o.hz90, true, "90 Hz");
            });
            consent(ui, app, needs_consent);
        }
        ToolId::WalkTest => {
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut o.walk_ae, "Auto exposure (0x51, approved)");
                ui.checkbox(&mut o.walk_euroc, "Record the walk for replay (EuRoC)");
                ui.checkbox(&mut o.walk_record, format!("Raw camera recording (~{})", gib(recording_bytes(o.walk_secs))));
                if o.walk_record {
                    ui.label("seconds");
                    ui.add(egui::Slider::new(&mut o.walk_secs, 30..=300));
                }
            });
            consent(ui, app, needs_consent);
        }
        ToolId::CtrlRecording => {
            ui.horizontal(|ui| {
                ui.label("Seconds");
                ui.add(egui::Slider::new(&mut o.rec_secs, 30..=180));
                ui.label(RichText::new(format!("~{}", gib(recording_bytes(o.rec_secs)))).color(pal(ui).muted));
            });
        }
        _ => {}
    }
}

fn steam(ui: &mut Ui, app: &mut PimaxApp, busy: bool, click: &mut Option<Click>) {
    let o = app.core.facts.openvr;
    ui.horizontal_wrapped(|ui| {
        let state = match (o.exists, o.xrizer, o.read_only) {
            (false, _, _) => ("no OpenVR runtime file", pal(ui).muted),
            (true, true, true) => ("xrizer + Monado (file read-only)", pal(ui).ok),
            (true, true, false) => ("xrizer + Monado (file writable: SteamVR may rewrite it)", pal(ui).warn),
            (true, false, _) => ("SteamVR", pal(ui).info),
        };
        chip(ui, state.0, state.1);
    });
    ui.horizontal(|ui| {
        if ui.add_enabled(!busy, egui::Button::new("Use xrizer + Monado")).clicked() {
            *click = Some(Click::Runtime("xrizer"));
        }
        if ui.add_enabled(!busy, egui::Button::new("Restore SteamVR")).clicked() {
            *click = Some(Click::Runtime("steamvr"));
        }
        if ui.add_enabled(!busy, egui::Button::new("Show status")).clicked() {
            *click = Some(Click::Runtime("status"));
        }
    });
    ui.label("Steam launch options (Properties > General > Launch Options):");
    widgets::code_box(ui, &[app.core.steam_options()]);
    note(ui, "Start desk/VR first; the app never launches games. Output of the switch goes to the Output panel below.");
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        chip(ui, "Proton games", pal(ui).warn);
        widgets::row_tag(ui, Some(27));
    });
    note(
        ui,
        "Every Steam VR game needs the line above in its Launch Options (Steam > game > Properties > General), once: without it Steam's container does not see Monado and the game runs flat. Then start VR here and press Play in Steam; SteamVR mode (no -openxr) is the default.",
    );
}

/// The running walk: elapsed time and the 6DoF state from monado.log, and Finish.
fn walk_panel(ui: &mut Ui, app: &mut PimaxApp, busy: bool, click: &mut Option<Click>) {
    let p = pal(ui);
    let live = &app.core.live;
    let six = &live.log.sixdof;
    ui.horizontal_wrapped(|ui| {
        let up = app.core.session_tool.as_ref().is_some_and(|s| s.opened);
        chip(ui, if up { "walk running" } else { "starting" }, if up { p.ok } else { p.info });
        if let Some(age) = live.service.as_ref().and_then(|s| s.age) {
            ui.label(format!("for {}", fmt_age(age)));
        }
        let (t, c) = match six.state {
            TrackState::Tracking => ("6DoF tracking", p.ok),
            TrackState::Lost => ("lost (3DoF, position held)", p.fail),
            TrackState::Waiting => ("waiting for the tracker", p.warn),
            TrackState::Off => ("no 6DoF yet", p.muted),
        };
        chip(ui, t, c);
        ui.label(format!(
            "{} acquired, {} lost, {} resets, {} frame sets{}",
            six.acquisitions,
            six.losses,
            six.resets,
            six.frame_sets,
            six.correction.map(|(mm, deg)| format!(", correction {mm:.1} mm / {deg:.2} deg")).unwrap_or_default()
        ));
    });
    note(ui, "Put the headset on: stand still 5 s, walk slowly, end where you started, then Finish.");
    if ui.add_enabled(!busy, egui::Button::new("Finish walk (stops desk mode: pimax desk stop --grace 30)")).clicked() {
        *click = Some(Click::FinishWalk);
    }
}
