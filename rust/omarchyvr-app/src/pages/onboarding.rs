//! Onboarding: the headset module's steps (find the headset, check this PC, fix, udev, IPD, pairing, eye calibration,
//! room, done) as a stepper. Every step has Run (its action), Next (once its condition holds), Skip and Back; the
//! progress is kept in `~/.local/state/omarchyvr/onboarding.state`, so the page resumes where it was left, and Start over
//! deletes it. The actions are the existing tools and fixes; nothing here talks to the headset.
use crate::app::{PimaxApp, SetupTab};
use crate::pages::{checks, home};
use crate::theme::{self, icon};
use crate::widgets::{self, card, chip, kv, note, pal, spinner, title};
use eframe::egui::{self, RichText, Ui};
use omarchyvr_gui_core::app::Page;
use omarchyvr_gui_core::device::onboarding::{State, Step};
use omarchyvr_gui_core::device::Severity;
use omarchyvr_gui_core::headsets::{self, Detected};
use omarchyvr_gui_core::onboarding::{intro, machine_rows, run_label};
use omarchyvr_gui_core::tools::ToolId;

fn state_color(ui: &Ui, st: State, current: bool) -> egui::Color32 {
    let p = theme::pal(ui);
    match st {
        _ if current => p.text,
        State::Done => p.ok,
        State::Skipped => p.faint,
        State::Failed => p.fail,
        State::Pending => p.dim,
    }
}

/// Draw the page.
pub fn show(ui: &mut Ui, app: &mut PimaxApp) {
    app.core.want_onboarding();
    if !app.core.onboarding.loaded {
        card(ui, |ui| {
            ui.horizontal(|ui| {
                spinner(ui);
                ui.label(RichText::new("Reading the onboarding progress...").color(pal(ui).muted));
            });
        });
        return;
    }
    let steps = app.core.onboarding_steps();
    let step = app.core.onboarding_step();
    let module = app.core.onboarding_module();
    card(ui, |ui| {
        ui.horizontal(|ui| {
            title(ui, "Set up your headset");
            widgets::help(ui, &format!("{}: each step can be run, skipped and resumed later.", module.descriptor().name));
            if module.is_mock() {
                chip(ui, "not supported yet", pal(ui).warn);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::ismall(ui, icon::ARROW_COUNTER_CLOCKWISE, "Start over").on_hover_text("Forget every step (deletes ~/.local/state/omarchyvr/onboarding.state)").clicked() {
                    app.core.onboarding_restart();
                }
            });
        });
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            for (i, s) in steps.iter().enumerate() {
                let st = app.core.onboarding.progress.state(*s);
                let mark = match st {
                    State::Done => icon::CHECK_CIRCLE,
                    State::Failed => icon::X_CIRCLE,
                    State::Skipped => icon::ARROW_SQUARE_OUT,
                    State::Pending => icon::CIRCLE,
                };
                let text = RichText::new(format!("{mark} {}. {}", i + 1, s.title())).size(12.5).color(state_color(ui, st, *s == step));
                let r = ui.selectable_label(*s == step, text);
                r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, s.title()));
                if r.clicked() {
                    app.core.onboarding_goto(*s);
                }
            }
        });
    });
    card(ui, |ui| {
        let st = app.core.onboarding.progress.state(step);
        ui.horizontal(|ui| {
            title(ui, step.title());
            match st {
                State::Done => chip(ui, "done", pal(ui).ok),
                State::Skipped => chip(ui, "skipped", pal(ui).muted),
                State::Failed => chip(ui, "failed: try again", pal(ui).fail),
                State::Pending => {}
            }
        });
        note(ui, intro(step));
        ui.add_space(6.0);
        body(ui, app, step);
        ui.add_space(10.0);
        let blocker = app.core.onboarding_blocker(step);
        if let Some(b) = &blocker {
            ui.label(RichText::new(b).color(pal(ui).warn));
        }
        let busy = app.core.busy();
        ui.horizontal_wrapped(|ui| {
            if let Some(label) = run_label(step) {
                let ic = if matches!(step, Step::Detect | Step::Checks | Step::Fixes) { icon::ARROWS_CLOCKWISE } else { icon::PLAY };
                if widgets::ibutton(ui, ic, label, !busy).clicked() {
                    app.core.onboarding_run(step);
                }
            }
            if step == Step::Done {
                if widgets::primary(ui, "Finish", true).clicked() {
                    app.core.finish_onboarding();
                }
            } else {
                let r = widgets::primary(ui, "Next", blocker.is_none());
                let r = match &blocker {
                    Some(b) => r.on_disabled_hover_text(b.as_str()),
                    None => r.on_hover_text("Mark this step done"),
                };
                if r.clicked() {
                    app.core.onboarding_next();
                }
                if widgets::tbutton(ui, "Skip", true).on_hover_text("Go on; the step stays listed as skipped").clicked() {
                    app.core.onboarding_skip();
                }
            }
            let first = steps.first() == Some(&step);
            if widgets::tbutton(ui, "Back", !first).clicked() {
                app.core.onboarding_back();
            }
        });
    });
}

/// The body of a step.
fn body(ui: &mut Ui, app: &mut PimaxApp, step: Step) {
    match step {
        Step::Detect => detect(ui, app),
        Step::Checks => {
            let Some(f) = app.core.facts.findings.clone() else {
                ui.horizontal(|ui| {
                    spinner(ui);
                    ui.label("Checking this PC...");
                });
                return;
            };
            let (ok, warn, fail) = omarchyvr_gui_core::findings::counts(&f);
            ui.horizontal_wrapped(|ui| {
                chip(ui, &format!("{ok} ok"), pal(ui).ok);
                chip(ui, &format!("{warn} warnings"), pal(ui).warn);
                chip(ui, &format!("{fail} failing"), pal(ui).fail);
            });
            egui::Grid::new("onb-areas").num_columns(2).spacing([18.0, 4.0]).show(ui, |ui| {
                for (area, list) in omarchyvr_gui_core::findings::grouped(&f) {
                    let bad: Vec<&str> = list.iter().filter(|x| x.severity >= Severity::Warn).map(|x| x.title.as_str()).collect();
                    let v = if bad.is_empty() { RichText::new("ok").color(pal(ui).ok) } else { RichText::new(bad.join(", ")).color(pal(ui).warn) };
                    kv(ui, omarchyvr_gui_core::findings::area_title(area), v);
                }
            });
            if widgets::ibutton(ui, icon::WRENCH, "Open the checks", true).clicked() {
                app.core.page = Page::Setup;
                app.ui.setup_tab = SetupTab::Checks;
            }
        }
        Step::Fixes => {
            let f = app.core.facts.findings.clone().unwrap_or_default();
            let open: Vec<_> = f.iter().filter(|x| omarchyvr_gui_core::findings::fixable(x)).cloned().collect();
            if open.is_empty() {
                ui.label(RichText::new(format!("{} Nothing to fix.", icon::CHECK_CIRCLE)).color(pal(ui).ok));
            }
            let mut run = None;
            for x in &open {
                if let Some(p) = checks::finding_ui(ui, app, x) {
                    run = Some(p);
                }
                ui.add_space(4.0);
            }
            if let Some(p) = run {
                checks::run_fix(app, p);
            }
            note(ui, "Builds, the launcher and this app are on Setup > Install.");
        }
        Step::Udev if app.core.onboarding.install.plan_text.is_some() => {
            let done = app.core.onboarding.install.root_done;
            ui.label(
                RichText::new(if done { "Done: the system part of omarchyvr is set up." } else { "Not set up yet. This is everything that changes, as root, with one password:" })
                    .color(if done { pal(ui).ok } else { pal(ui).warn }),
            );
            if let Some(t) = &app.core.onboarding.install.plan_text {
                widgets::code_box(ui, &t.lines().map(str::to_string).collect::<Vec<_>>());
            }
            note(ui, "Undo: Ready > Uninstall omarchyvr (or ./uninstall.sh). When the list says reboot, reboot once afterwards.");
            crate::pages::setup::nvidia_card(ui, app);
        }
        Step::Udev => {
            let (text, c) = match app.core.facts.udev_rule {
                Some(true) => ("The rule is installed and matches the repository's.", pal(ui).ok),
                Some(false) => ("An older rule is installed.", pal(ui).warn),
                None => ("No rule is installed.", pal(ui).warn),
            };
            ui.label(RichText::new(text).color(c));
            if let Ok(p) = omarchyvr_gui_core::findings::udev_fix(&app.core.paths.repo) {
                ui.label("The fix (root, after you confirm it):");
                widgets::code_box(ui, &p.commands);
                egui::CollapsingHeader::new(RichText::new("Undo").small()).id_salt("onb-udev-undo").show(ui, |ui| {
                    widgets::code_box(ui, &p.undo);
                });
            }
        }
        Step::Ipd => {
            match app.core.facts.ipd_mm {
                Some(mm) => ui.label(RichText::new(format!("Saved IPD: {mm:.1} mm (~/.config/omarchyvr/ipd.env)")).color(pal(ui).ok)),
                None => ui.label("No IPD saved yet."),
            };
            tool_blockers(ui, app, ToolId::Ipd);
        }
        Step::Pair => {
            note(ui, "Pairing goes through auto mode (one approved StartPair per try); a running desk or VR session is needed to see the controllers.");
            for side in ["left", "right"] {
                ui.horizontal(|ui| {
                    let i = usize::from(side == "right");
                    let seen = app.core.onboarding.paired[i]
                        || app.core.live.log.ctrl.slots_seen.contains(&if i == 0 { omarchyvr_gui_core::devices::SLOT_LEFT } else { omarchyvr_gui_core::devices::SLOT_RIGHT });
                    ui.label(RichText::new(format!("{} {} controller", icon::GAME_CONTROLLER, if i == 0 { "Left" } else { "Right" })).strong());
                    if seen {
                        chip(ui, "paired", pal(ui).ok);
                    }
                    home::pair_ui(ui, app, if i == 0 { "left" } else { "right" });
                });
            }
        }
        Step::EyeCal => {
            if app.core.facts.eye_calibration {
                let acc = app.core.facts.eye_accuracy_deg.map(|a| format!(", accuracy {a:.1} deg")).unwrap_or_default();
                ui.label(RichText::new(format!("Saved calibration (~/.config/omarchyvr/eye-calibration.toml{acc})")).color(pal(ui).ok));
            } else {
                ui.label("No eye calibration saved yet.");
            }
            tool_blockers(ui, app, ToolId::EyeCalibration);
            if app.core.session_tool.is_some() && widgets::ibutton(ui, icon::STOP, "Stop the calibration session", !app.core.busy()).clicked() {
                app.core.stop_tool_session();
            }
        }
        Step::Room => {
            app.core.want_room();
            let maps = app.core.room.state.maps.clone();
            if maps.is_empty() {
                ui.label("No room map yet.");
            } else {
                ui.label(RichText::new(format!("Maps: {}", maps.join(", "))).color(pal(ui).ok));
            }
            note(ui, "Open Room setup shows the Room page (map, recognise, AprilTag).");
        }
        Step::Done => {
            let steps = app.core.onboarding_steps();
            egui::Grid::new("onb-done").num_columns(2).spacing([18.0, 4.0]).show(ui, |ui| {
                for s in steps.iter().filter(|s| **s != Step::Done) {
                    let st = app.core.onboarding.progress.state(*s);
                    let v = match st {
                        State::Done => RichText::new("done").color(pal(ui).ok),
                        State::Skipped => RichText::new("skipped").color(pal(ui).muted),
                        State::Failed => RichText::new("failed").color(pal(ui).fail),
                        State::Pending => RichText::new("not done").color(pal(ui).warn),
                    };
                    kv(ui, s.title(), v);
                }
            });
        }
    }
}

/// Why a tool cannot start now.
fn tool_blockers(ui: &mut Ui, app: &PimaxApp, t: ToolId) {
    let a = app.core.tool_availability(t);
    for b in &a.blockers {
        ui.label(RichText::new(&b.text).size(12.0).color(pal(ui).warn));
    }
}

/// The first step: what this PC is and which headset.
fn detect(ui: &mut Ui, app: &mut PimaxApp) {
    let found = headsets::detect(&app.core.facts);
    let (line, c) = match &found {
        Detected::Module(m) => (format!("Found: {} (USB {})", m.descriptor().name, headsets::usb_ids(m.descriptor())), pal(ui).ok),
        Detected::Possibly(m) => (format!("Possibly {} (not supported yet)", m.descriptor().name), pal(ui).warn),
        Detected::None if app.core.facts.at == 0 => ("Looking at the USB devices...".to_string(), pal(ui).muted),
        Detected::None => ("No known headset on USB. Plug it in (USB-C and DisplayPort), or pick it below.".to_string(), pal(ui).warn),
    };
    ui.label(RichText::new(line).color(c).strong());
    let suggested = match &found {
        Detected::Module(m) | Detected::Possibly(m) => Some(m.id()),
        Detected::None => None,
    };
    let chosen = app.ui.onb_pick.or(app.core.onboarding.progress.module.as_deref().and_then(headsets::by_id).map(|m| m.id())).or(suggested).unwrap_or_else(|| headsets::default_module().id());
    ui.add_space(4.0);
    for m in headsets::all() {
        let d = m.descriptor();
        let id: &'static str = m.id();
        let label = if m.is_mock() { format!("{} (not supported yet)", d.name) } else { d.name.clone() };
        if ui.radio(id == chosen, label).clicked() {
            app.ui.onb_pick = Some(id);
        }
    }
    let name = headsets::by_id(chosen).map(|m| m.descriptor().name.clone()).unwrap_or_default();
    if widgets::primary(ui, &format!("Use {name}"), true).clicked() {
        app.core.onboarding_use_module(chosen);
        app.ui.onb_pick = None;
    }
    ui.add_space(6.0);
    egui::CollapsingHeader::new("This PC").id_salt("onb-pc").default_open(false).show(ui, |ui| {
        egui::Grid::new("onb-machine").num_columns(2).striped(true).spacing([18.0, 4.0]).show(ui, |ui| {
            for (k, v) in machine_rows(&app.core.facts, &app.core.live) {
                kv(ui, &k, v);
            }
        });
    });
}
