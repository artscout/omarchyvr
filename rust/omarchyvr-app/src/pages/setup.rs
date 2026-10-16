//! Setup, in three tabs: Checks (the pimax-device findings by area, with catalogue fixes and the applied-fix list),
//! Install (the checklist: omarchyvr-doctor + the LESSONS checks, builds, launcher, this app) and Autofix. User fixes run
//! directly with a progress log; root fixes show the exact lines with Copy and "Run with pkexec" (confirmed).
use crate::app::{PimaxApp, SetupTab};
use crate::pages::{autofix, checks};
use crate::theme;
use crate::widgets::{self, card, chip, level_chip, note, pal};
use eframe::egui::{self, RichText, Ui};
use omarchyvr_doctor::Level;
use omarchyvr_gui_core::checklist::{counts, Fix, Group, Item};

/// What a click on an item asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemClick {
    /// Run the user fix.
    User(String),
    /// Show the pkexec confirmation.
    Root(String),
}

/// One checklist item with its fix. Returns a click.
pub fn item_ui(ui: &mut Ui, item: &Item, busy: bool, running: bool, focus: bool, scroll: bool) -> Option<ItemClick> {
    let mut click = None;
    let p = pal(ui);
    let frame = egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(6, 4))
        .corner_radius(egui::CornerRadius::same(6))
        .stroke(if focus { egui::Stroke::new(2.0, p.accent) } else { egui::Stroke::NONE });
    let r = frame.show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            level_chip(ui, item.level);
            ui.label(RichText::new(&item.title).strong());
            widgets::row_tag(ui, item.row);
            if item.from_doctor {
                ui.label(RichText::new("doctor").small().color(p.muted)).on_hover_text("from omarchyvr-doctor");
            }
        });
        ui.indent(("detail", &item.id), |ui| {
            note(ui, &item.detail);
            if item.level != Level::Ok {
                match &item.fix {
                    Fix::None => {}
                    Fix::User(u) => {
                        note(ui, &u.why);
                        let lines: Vec<String> = u.steps.iter().map(|c| c.display()).collect();
                        widgets::code_box(ui, &lines);
                        ui.horizontal(|ui| {
                            let blocked = u.needs_idle && running;
                            let b = widgets::primary(ui, "Fix", !busy && !blocked);
                            let b = if blocked { b.on_disabled_hover_text("Stop desk/VR first (LESSONS row 8)") } else { b };
                            if b.clicked() {
                                click = Some(ItemClick::User(item.id.clone()));
                            }
                            if u.follow_up.is_some() {
                                ui.label(RichText::new("then the setcap prompt (the relink drops cap_sys_nice)").small().color(p.muted));
                            }
                        });
                    }
                    Fix::Root(r) => {
                        note(ui, &r.why);
                        widgets::code_box(ui, &r.lines);
                        ui.horizontal(|ui| {
                            chip(ui, "root", p.warn);
                            match &r.pkexec {
                                Ok(()) => {
                                    if ui.add_enabled(!busy, egui::Button::new("Run with pkexec...")).on_hover_text("Shows the exact command first; polkit asks for your password").clicked() {
                                        click = Some(ItemClick::Root(item.id.clone()));
                                    }
                                }
                                Err(why) => {
                                    ui.label(RichText::new(format!("copy only: {why}")).small().color(p.muted));
                                }
                            }
                            if r.reboot {
                                ui.label(RichText::new("then reboot").small().color(p.warn));
                            }
                        });
                    }
                    Fix::Manual { why, lines } => {
                        note(ui, why);
                        if !lines.is_empty() {
                            widgets::code_box(ui, lines);
                        }
                    }
                }
            }
        });
    });
    if scroll {
        r.response.scroll_to_me(Some(egui::Align::Center));
    }
    click
}

/// Draw the page: the tabs (developer mode), then the tab.
pub fn show(ui: &mut Ui, app: &mut PimaxApp) {
    // the simple flow shows only the ready check; Install and Autofix are Developer mode
    let dev = app.core.state.dev_mode;
    if dev {
        const TABS: [(SetupTab, &str); 3] = [(SetupTab::Checks, "Checks"), (SetupTab::Install, "Install"), (SetupTab::Autofix, "Autofix")];
        let names: Vec<&str> = TABS.iter().map(|t| t.1).collect();
        let cur = TABS.iter().position(|t| t.0 == app.ui.setup_tab).unwrap_or(0);
        if let Some(i) = widgets::seg(ui, "setup-tabs", &names, cur, true) {
            app.ui.setup_tab = TABS[i].0;
        }
    }
    if app.core.setup_focus.is_some() && dev {
        // a preflight or the onboarding pointed at an install item
        app.ui.setup_tab = SetupTab::Install;
    }
    let tab = if dev { app.ui.setup_tab } else { SetupTab::Checks };
    match tab {
        SetupTab::Checks => checks::show(ui, app),
        SetupTab::Install => install(ui, app),
        SetupTab::Autofix => autofix::show(ui, app),
    }
}

/// The install checklist.
pub fn install(ui: &mut Ui, app: &mut PimaxApp) {
    let items = app.core.items.clone();
    let (ok, warn, fail) = counts(&items);
    let busy = app.core.busy();
    let running = app.core.live.anything_running();
    widgets::section_with(
        ui,
        "Install and builds",
        Some("omarchyvr-doctor's checks plus the rules from docs/LESSONS.md. Fixes that need root are shown first and only run when you press Run with pkexec and confirm."),
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
            if widgets::tbutton(ui, "Re-check", !busy).clicked() {
                app.core.refresh(true);
            }
            let r = widgets::tbutton(ui, "Rebuild Monado (desk)", !busy && !running)
                .on_hover_text("cmake -DPIMAX_CAM=ON . and ninja in ~/src/monado-eye/build, then the setcap prompt");
            let r = if running { r.on_disabled_hover_text("Stop desk/VR first (LESSONS row 8)") } else { r };
            if r.clicked() {
                app.core.rebuild_desk();
            }
            if widgets::tbutton(ui, "Copy doctor fix plan", true).on_hover_text("omarchyvr-doctor --fix-plan as text").clicked() {
                if let Some(rep) = &app.core.facts.report {
                    ui.ctx().copy_text(rep.fix_plan());
                    app.core.message = "Fix plan copied".into();
                }
            }
        });
        },
    );
    if let Some(f) = app.core.setup_focus.take() {
        app.ui.focus = Some((f, std::time::Instant::now(), true));
    }
    let (focus, scroll) = match &mut app.ui.focus {
        Some((id, t, first)) if t.elapsed() < std::time::Duration::from_secs(5) => {
            let s = *first;
            *first = false;
            (Some(id.clone()), s)
        }
        _ => (None, false),
    };
    let mut click = None;
    for g in Group::ALL {
        let group: Vec<&Item> = items.iter().filter(|i| i.group == g).collect();
        if group.is_empty() {
            continue;
        }
        let bad = group.iter().filter(|i| i.bad()).count();
        let worst = group.iter().map(|i| i.level).max().unwrap_or(Level::Ok);
        card(ui, |ui| {
            let c = if worst >= Level::Warn { widgets::level_color(ui, worst) } else { theme::pal(ui).text };
            let head = theme::caps(&format!("{}{}", g.title(), if bad > 0 { format!("  ·  {bad} to look at") } else { String::new() }), 13.0, c).family(theme::heading(ui));
            let has_focus = scroll && focus.as_ref().is_some_and(|f| group.iter().any(|i| &i.id == f));
            egui::CollapsingHeader::new(head)
                .id_salt(("group", g as u8))
                .default_open(bad > 0)
                .open(has_focus.then_some(true))
                .show(ui, |ui| {
                    for it in &group {
                        let f = focus.as_deref() == Some(it.id.as_str());
                        if let Some(c) = item_ui(ui, it, busy, running, f, f && scroll) {
                            click = Some(c);
                        }
                        ui.add_space(2.0);
                    }
                });
        });
    }
    match click {
        Some(ItemClick::User(id)) => app.core.run_user_fix(&id),
        Some(ItemClick::Root(id)) => app.core.ask_root(&id),
        None => {}
    }
    widgets::section(ui, "Desktop entry", Some("Launcher and app > This app installs the binary to ~/.local/bin/omarchyvr-app, the icon and 'Pimax' in ~/.local/share/applications (no root)."), |ui| {
        note(ui, "~/.local/bin/omarchyvr-app, \"omarchyvr\" in the app launcher");
    });
}
