//! Setup > Checks: the pimax-device checks (its own and every pimax-doctor check through `DoctorBridge`) grouped by
//! area, each with its catalogue fix. User fixes run as a job with their output shown; root fixes open the pkexec
//! dialog with the exact lines (nothing runs before the user confirms there); reboots are only shown. Every fix the
//! app ran is listed under "Applied fixes" (`~/.local/state/pimax/fixes.log`) with Undo.
use crate::app::PimaxApp;
use crate::theme::{self, icon};
use crate::widgets::{self, card, chip, note, pal, spinner};
use eframe::egui::{self, RichText, Ui};
use pimax_gui_core::device::check::Planned;
use pimax_gui_core::device::{Finding, Severity};
use pimax_gui_core::findings::{self, CatalogueRun, FixAction};

/// Colour of a finding severity.
pub fn sev_color(ui: &Ui, s: Severity) -> egui::Color32 {
    let p = pal(ui);
    match s {
        Severity::Ok => p.ok,
        Severity::Info => p.info,
        Severity::Warn => p.warn,
        Severity::Fail => p.fail,
    }
}

/// Severity chip.
pub fn sev_chip(ui: &mut Ui, s: Severity) {
    chip(ui, &s.id().to_uppercase(), sev_color(ui, s));
}

/// What a planned fix shows and offers. Returns true when its button was pressed.
pub fn planned_ui(ui: &mut Ui, app: &PimaxApp, p: &Planned, id_salt: &str, button: &str) -> bool {
    let colors = pal(ui);
    let mut pressed = false;
    note(ui, p.fix.why);
    widgets::code_box(ui, &p.commands);
    let busy = app.core.busy();
    let run = CatalogueRun { planned: p.clone(), undo: false };
    let action = findings::action(&run, &app.core.launcher_bin());
    let manual = matches!(action, FixAction::Manual(..));
    ui.horizontal_wrapped(|ui| {
        match action {
            FixAction::User(_) => {
                if widgets::primary(ui, button, !busy).on_hover_text("Runs as your user; the output shows below").clicked() {
                    pressed = true;
                }
            }
            FixAction::Root(r) => {
                chip(ui, "root", colors.warn);
                let label = format!("{button} with pkexec...");
                match &r.pkexec {
                    Ok(()) => {
                        if ui.add_enabled(!busy, egui::Button::new(label)).on_hover_text("Shows the exact command first; polkit asks for your password").clicked() {
                            pressed = true;
                        }
                    }
                    Err(why) => {
                        ui.label(RichText::new(format!("copy only: {why}")).small().color(colors.muted));
                    }
                }
            }
            FixAction::Manual(why, _) => {
                ui.label(RichText::new(why).small().color(colors.muted));
            }
        }
        if p.fix.reboot && !manual {
            ui.label(RichText::new("then reboot").small().color(colors.warn));
        }
    });
    if !p.undo.is_empty() {
        egui::CollapsingHeader::new(RichText::new("Undo").small()).id_salt(("undo", id_salt)).show(ui, |ui| {
            widgets::code_box(ui, &p.undo);
        });
    }
    pressed
}

/// One finding. Returns the fix to run when its button was pressed.
pub fn finding_ui(ui: &mut Ui, app: &PimaxApp, f: &Finding) -> Option<Planned> {
    let mut run = None;
    ui.horizontal_wrapped(|ui| {
        sev_chip(ui, f.severity);
        ui.label(RichText::new(&f.title).strong());
        ui.label(RichText::new(&f.check).size(10.5).color(theme::pal(ui).faint));
    });
    ui.indent(("finding", &f.check, &f.title), |ui| {
        note(ui, &f.detail);
        if f.severity >= Severity::Warn {
            match findings::plan(f) {
                Some(Ok(p)) => {
                    if planned_ui(ui, app, &p, &format!("{}-{}", f.check, f.title), "Fix") {
                        run = Some(p);
                    }
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(format!("This fix cannot be shown: {e}")).small().color(pal(ui).fail));
                }
                None => {}
            }
        }
    });
    run
}

/// Run a planned fix through the core (user job, or the pkexec dialog).
pub fn run_fix(app: &mut PimaxApp, p: Planned) {
    let title = p.fix.id.to_string();
    app.core.run_catalogue(CatalogueRun { planned: p, undo: false }, &title);
}

/// Draw the tab.
pub fn show(ui: &mut Ui, app: &mut PimaxApp) {
    app.core.want_fixes();
    let Some(all) = app.core.facts.findings.clone() else {
        card(ui, |ui| {
            ui.horizontal(|ui| {
                spinner(ui);
                ui.label(RichText::new("Checking this PC...").color(pal(ui).muted));
            });
        });
        return;
    };
    let (ok, warn, fail) = findings::counts(&all);
    let busy = app.core.busy();
    widgets::section_with(
        ui,
        "System checks",
        Some("Checks the headset, USB, GPU, CPU and runtime. Fix runs the repair; a fix that needs root shows its lines and asks first."),
        |ui| {
            chip(ui, &format!("{ok} ok"), pal(ui).ok);
            if warn > 0 {
                chip(ui, &format!("{warn} warnings"), pal(ui).warn);
            }
            if fail > 0 {
                chip(ui, &format!("{fail} failing"), pal(ui).fail);
            }
        },
        |ui| {
            ui.horizontal_wrapped(|ui| {
                if widgets::ibutton(ui, icon::ARROWS_CLOCKWISE, "Check again", !busy).clicked() {
                    app.core.refresh(true);
                }
                if widgets::ibutton(ui, icon::ROBOT, "Ask Autofix", true).on_hover_text("Let a language model read the findings and propose catalogue fixes").clicked() {
                    app.ui.setup_tab = crate::app::SetupTab::Autofix;
                }
            });
        },
    );
    super::gamecheck::show(ui, app);
    if app.core.headset_camera_missing() || app.core.restart_headset_progress().is_some() {
        card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                sev_chip(ui, Severity::Warn);
                ui.label(RichText::new("Headset camera missing").strong());
                ui.label(RichText::new("usb 34a4:0014").size(10.5).color(theme::pal(ui).faint));
            });
            note(ui, "The headset is on USB but its camera is not (no passthrough, no 6DoF). Restart headset brings it back in about 30 seconds (not while VR runs).");
            let blocker = app.core.restart_headset_blocker();
            let r = widgets::primary(ui, "Restart headset", blocker.is_none());
            let r = match &blocker {
                Some(b) => r.on_disabled_hover_text(b.as_str()),
                None => r.on_hover_text("pimax vr reset-headset"),
            };
            if r.clicked() {
                app.core.restart_headset();
            }
            crate::pages::home::reset_state(ui, app);
        });
    }
    let mut run = None;
    for (area, list) in findings::grouped(&all) {
        let bad = list.iter().filter(|f| f.severity >= Severity::Warn).count();
        let worst = list.iter().map(|f| f.severity).max().unwrap_or(Severity::Ok);
        card(ui, |ui| {
            let color = if worst >= Severity::Warn { sev_color(ui, worst) } else { theme::pal(ui).text };
            let head = theme::caps(&format!("{}{}", findings::area_title(area), if bad > 0 { format!("  ·  {bad} to look at") } else { String::new() }), 13.0, color).family(theme::heading(ui));
            egui::CollapsingHeader::new(head).id_salt(("area", area.id())).default_open(bad > 0).show(ui, |ui| {
                for f in &list {
                    if let Some(p) = finding_ui(ui, app, f) {
                        run = Some(p);
                    }
                    ui.add_space(4.0);
                }
            });
        });
    }
    if let Some(p) = run {
        run_fix(app, p);
    }
    applied(ui, app);
}

/// "Applied fixes": fixes.log with Undo.
pub fn applied(ui: &mut Ui, app: &mut PimaxApp) {
    let entries = app.core.fixes.entries.clone();
    let busy = app.core.busy();
    let mut undo = None;
    widgets::section(ui, "Applied fixes", Some("Every catalogue fix the app ran or opened, from ~/.local/state/pimax/fixes.log (newest first). Undo runs the fix's undo lines the same way: as your user, or through the pkexec dialog."), |ui| {
        if !app.core.fixes.loaded {
            spinner(ui);
            return;
        }
        if entries.is_empty() {
            ui.label(RichText::new("No fix applied yet.").color(pal(ui).muted));
        }
        for (i, e) in entries.iter().enumerate() {
            ui.horizontal_wrapped(|ui| {
                let c = if e.outcome.contains("failed") { pal(ui).fail } else { pal(ui).ok };
                ui.label(RichText::new(e.when()).size(11.5).color(theme::pal(ui).faint));
                ui.label(RichText::new(&e.id).strong());
                ui.label(RichText::new(&e.outcome).size(12.0).color(c));
                let back = if e.is_undo() { &e.commands } else { &e.undo };
                let label = if e.is_undo() { "Apply again" } else { "Undo" };
                let can = !busy && !back.is_empty() && e.planned().is_some();
                let r = widgets::ibutton(ui, icon::ARROW_COUNTER_CLOCKWISE, label, can);
                let r = if back.is_empty() { r.on_disabled_hover_text("This fix has nothing to undo") } else { r.on_hover_text(back.join("\n")) };
                if r.clicked() {
                    undo = Some(i);
                }
            });
            ui.add(egui::Label::new(RichText::new(e.commands.join(" && ")).monospace().size(11.0).color(theme::pal(ui).dim)).truncate());
            ui.add_space(2.0);
        }
    });
    if let Some(i) = undo {
        app.core.undo_logged(i);
    }
}
