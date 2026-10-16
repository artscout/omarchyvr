//! Setup > Autofix: (a) the backend (a local model found on 127.0.0.1, or Claude with your own key), (b) exactly what
//! would be sent, read-only, with its size, (c) Send and the answer: explanations, catalogue fixes to approve or
//! decline one at a time, rejected lines greyed, (d) Contribute: a local branch with a test report. Nothing leaves the
//! machine before Send; the key is kept in memory only.
use crate::app::PimaxApp;
use crate::pages::checks;
use crate::theme::{self, icon};
use crate::widgets::{self, card, chip, note, pal, spinner, title};
use eframe::egui::{self, RichText, Ui};
use pimax_gui_core::autofix::fmt_bytes;
use pimax_gui_core::device::autofix::Proposal;

/// Draw the tab.
pub fn show(ui: &mut Ui, app: &mut PimaxApp) {
    app.core.want_autofix();
    backend(ui, app);
    ui.add_space(8.0);
    preview(ui, app);
    ui.add_space(8.0);
    send(ui, app);
    ui.add_space(8.0);
    contribute(ui, app);
}

/// (a) The provider (from the AI tab).
fn backend(ui: &mut Ui, app: &mut PimaxApp) {
    let p = theme::pal(ui);
    card(ui, |ui| {
        ui.horizontal(|ui| {
            title(ui, "Autofix");
            if app.core.demo {
                chip(ui, "demo: nothing is sent", pal(ui).info);
            }
        });
        note(ui, "A language model reads the redacted check results and logs, explains what looks wrong and may propose fixes, but only from the app's fixed catalogue. You approve each fix; root fixes still go through the pkexec dialog.");
        ui.add_space(6.0);
        widgets::caption(ui, "Provider");
        match app.core.ai_resolve(pimax_config::ai::Feature::Autofix) {
            Ok(r) if app.core.ai.loaded => {
                let b = pimax_gui_core::llm::Backend::from_resolved(&r);
                let where_ = if b.local() { "stays on this PC" } else { "the bundle leaves this PC when you press Send" };
                ui.label(RichText::new(format!("{} {} ({where_})", icon::ROBOT, b.label())).color(p.text));
                let key = match r.key_source {
                    pimax_config::ai::KeySource::File => "key from ai.toml",
                    pimax_config::ai::KeySource::Env(_) => "key from the environment",
                    pimax_config::ai::KeySource::Session => "key kept for this session only",
                    pimax_config::ai::KeySource::None => "no key needed",
                };
                ui.label(RichText::new(key).size(12.0).color(p.dim));
            }
            Ok(_) => {
                ui.horizontal(|ui| {
                    spinner(ui);
                    ui.label("Reading the AI settings...");
                });
            }
            Err(e) => {
                ui.label(RichText::new(e).color(p.warn));
            }
        }
        if widgets::ibutton(ui, icon::ROBOT, "Change in the AI tab", true).on_hover_text("Providers, keys and which one Autofix uses").clicked() {
            app.core.page = pimax_gui_core::app::Page::Ai;
        }
    });
}

/// (b) The exact text, read-only.
fn preview(ui: &mut Ui, app: &mut PimaxApp) {
    let bytes = app.core.autofix_bytes();
    let building = app.core.autofix.building;
    card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            title(ui, "What will be sent");
            if let Some(n) = bytes {
                chip(ui, &fmt_bytes(n), pal(ui).info);
            }
            if widgets::ismall(ui, icon::ARROWS_CLOCKWISE, "Rebuild").on_hover_text("Collect the check results and logs again").clicked() {
                app.core.autofix_build();
            }
        });
        note(ui, "Exactly this text: the instructions, the list of fixes the model may propose, and the diagnostic bundle (check results, versions, the end of the session logs). Home directory, user and host names, serial numbers, machine id, e-mail and IP addresses and API keys are replaced first.");
        let Some(pv) = app.core.autofix.preview.clone() else {
            ui.horizontal(|ui| {
                spinner(ui);
                ui.label(if building { "Collecting..." } else { "Waiting for the first check of this PC..." });
            });
            return;
        };
        for (salt, name, text) in [("af-system", "Instructions", &pv.system), ("af-user", "Fix list and bundle", &pv.user)] {
            egui::CollapsingHeader::new(format!("{name} ({})", fmt_bytes(text.len()))).id_salt(salt).show(ui, |ui| {
                let p = theme::pal(ui);
                let row_h = ui.text_style_height(&egui::TextStyle::Monospace).max(13.0);
                let n = text.lines().count();
                widgets::inset(ui, |ui| {
                    egui::ScrollArea::vertical().id_salt((salt, "scroll")).max_height(320.0).auto_shrink([false, true]).show_rows(ui, row_h, n, |ui, range| {
                        for l in text.lines().skip(range.start).take(range.len()) {
                            ui.add(egui::Label::new(RichText::new(l).monospace().size(11.5).color(p.dim)).truncate());
                        }
                    });
                });
                if widgets::ismall(ui, icon::COPY, "Copy").clicked() {
                    ui.ctx().copy_text(text.clone());
                }
            });
        }
    });
}

/// (c) Send and the answer.
fn send(ui: &mut Ui, app: &mut PimaxApp) {
    let blocker = app.core.autofix_blocker();
    let backend = app.core.autofix_backend();
    let mut decide = None;
    card(ui, |ui| {
        let p = theme::pal(ui);
        title(ui, "Ask");
        if app.core.autofix.sending {
            ui.horizontal(|ui| {
                spinner(ui);
                ui.label("Waiting for the answer (up to a few minutes)...");
            });
        } else {
            let label = match &backend {
                Ok(b) if b.local() => format!("Send to {} (local)", b.label()),
                Ok(b) => format!("Send to {}", b.label()),
                Err(_) => "Send".to_string(),
            };
            let r = widgets::primary(ui, &label, blocker.is_none());
            let r = match &blocker {
                Some(b) => r.on_disabled_hover_text(b.as_str()),
                None => r.on_hover_text("The one moment the bundle above leaves the app"),
            };
            r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, blocker.is_none(), "Send"));
            if r.clicked() {
                app.core.autofix_send();
            }
            if let Some(b) = &blocker {
                ui.label(RichText::new(b).size(12.0).color(p.dim));
            }
        }
        if !app.core.autofix.last_request.is_empty() {
            ui.label(RichText::new(format!("Last request: {}", app.core.autofix.last_request)).size(11.5).color(p.faint));
        }
        let Some(ans) = app.core.autofix.answer.clone() else { return };
        ui.add_space(6.0);
        let a = match ans {
            Err(e) => {
                ui.label(RichText::new(e).color(p.fail));
                return;
            }
            Ok(a) => a,
        };
        ui.label(RichText::new(format!("Answer from {} in {:.0} s", a.model, a.secs)).size(12.0).color(p.dim));
        if let Some(n) = &a.note {
            ui.label(RichText::new(n).size(12.0).color(p.warn));
        }
        if a.proposals.is_empty() {
            ui.label(RichText::new("The answer was empty.").color(p.warn));
        }
        for (i, pr) in a.proposals.iter().enumerate() {
            match pr {
                Proposal::Explain(t) => {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(icon::INFO).color(p.info));
                        ui.add(egui::Label::new(t).wrap());
                    });
                }
                Proposal::Fix(planned) => {
                    widgets::inset(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(format!("{} {}", icon::WRENCH, planned.fix.id)).strong());
                            match a.decided[i] {
                                Some(true) => chip(ui, "approved", pal(ui).ok),
                                Some(false) => chip(ui, "declined", pal(ui).muted),
                                None => {}
                            }
                        });
                        let open = a.decided[i].is_none();
                        if checks::planned_ui(ui, app, planned, &format!("af-{i}"), "Approve") && open {
                            decide = Some((i, true));
                        }
                        if open && ui.button("Decline").clicked() {
                            decide = Some((i, false));
                        }
                    });
                }
                Proposal::Rejected { line, why } => {
                    ui.add(egui::Label::new(RichText::new(format!("{line}    (rejected: {why})")).monospace().size(11.5).color(p.faint)).wrap());
                }
            }
        }
    });
    if let Some((i, yes)) = decide {
        app.core.autofix_decide(i, yes);
    }
}

/// (d) Contribute: a local branch, printed share lines.
fn contribute(ui: &mut Ui, app: &mut PimaxApp) {
    let busy = app.core.busy();
    card(ui, |ui| {
        let p = theme::pal(ui);
        title(ui, "Contribute");
        note(ui, "Turn this machine's results into a test report others can use: a local git branch contrib/<date>-<headset> with reports/<date>-<headset>.toml and a PATCH.md (redacted). Nothing is uploaded: the push and the pull request are printed for you to run.");
        if widgets::ibutton(ui, icon::FILE_TEXT, "Prepare a contribution", true).clicked() {
            app.core.autofix_contribute();
        }
        match app.core.autofix.contrib.clone() {
            Some(Ok(c)) => {
                ui.label(RichText::new(format!("Written to {}", c.dir.display())).size(12.0).color(p.dim));
                if !c.made {
                    ui.label("Make the branch (runs these, your checkout is not touched):");
                    widgets::code_box(ui, &c.job.iter().map(|x| x.display()).collect::<Vec<_>>());
                    if widgets::ibutton(ui, icon::CODE, "Make the branch", !busy).clicked() {
                        app.core.autofix_make_branch();
                    }
                }
                ui.label(format!("Then share {} yourself:", c.branch));
                widgets::code_box(ui, &c.share);
            }
            Some(Err(e)) => {
                ui.label(RichText::new(e).color(p.fail));
            }
            None => {}
        }
    });
}
