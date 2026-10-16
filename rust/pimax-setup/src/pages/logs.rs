//! Logs & History: tail of the session logs (monado.log, broker, passthrough, WayVR, controller tracker, the 6DoF
//! test runs) and the history, open issues, lessons and safety rules from docs/INVENTORY.md and docs/LESSONS.md.
use crate::app::{LogsTab, PimaxApp};
use crate::widgets::{card, note, pal, spinner};
use eframe::egui::{self, RichText, Ui};
use pimax_gui_core::docs::Table;

fn loading(ui: &mut Ui, what: &str) {
    card(ui, |ui| {
        ui.horizontal(|ui| {
            spinner(ui);
            ui.label(RichText::new(what).color(pal(ui).muted));
        });
    });
}

/// Draw the page.
pub fn show(ui: &mut Ui, app: &mut PimaxApp) {
    card(ui, |ui| {
        ui.label(RichText::new("Logs & History").size(24.0).strong());
        ui.horizontal(|ui| {
            for (t, name) in [
                (LogsTab::Logs, "Logs"),
                (LogsTab::History, "Problems and fixes"),
                (LogsTab::Issues, "Open issues"),
                (LogsTab::Lessons, "Lessons"),
                (LogsTab::Safety, "Safety rules"),
            ] {
                if ui.selectable_label(app.ui.logs_tab == t, name).clicked() {
                    app.ui.logs_tab = t;
                }
            }
        });
    });
    ui.add_space(8.0);
    match app.ui.logs_tab {
        LogsTab::Logs => logs(ui, app),
        LogsTab::History => match app.core.docs().map(|d| d.history.clone()) {
            Some(t) => table_view(ui, app, "history", t, "docs/INVENTORY.md section 4"),
            None => loading(ui, "Reading docs/INVENTORY.md..."),
        },
        LogsTab::Issues => match app.core.docs().map(|d| d.open_issues.clone()) {
            Some(t) => table_view(ui, app, "issues", t, "docs/INVENTORY.md section 5.1"),
            None => loading(ui, "Reading docs/INVENTORY.md..."),
        },
        LogsTab::Lessons => {
            let Some(l) = app.core.docs().map(|d| d.lessons.clone()) else {
                loading(ui, "Reading docs/LESSONS.md...");
                return;
            };
            for (i, (sec, t)) in l.into_iter().enumerate() {
                card(ui, |ui| {
                    ui.label(RichText::new(&sec).strong().size(16.0));
                    grid(ui, &format!("lessons-{i}"), &t, "");
                });
                ui.add_space(6.0);
            }
            note(ui, "From docs/LESSONS.md: every row is enforced by a check, an ordering step or a state of this app (see docs/GUI.md).");
        }
        LogsTab::Safety => {
            let Some(s) = app.core.docs().map(|d| d.safety.clone()) else {
                loading(ui, "Reading docs/INVENTORY.md...");
                return;
            };
            for (sec, lines) in s {
                card(ui, |ui| {
                    ui.label(RichText::new(&sec).strong().size(16.0));
                    for l in lines {
                        ui.add(egui::Label::new(l).wrap());
                    }
                });
                ui.add_space(6.0);
            }
        }
    }
}

fn logs(ui: &mut Ui, app: &mut PimaxApp) {
    // the list and the tail are read in the background; this only draws what arrived
    app.core.want_logs(app.ui.log_source, app.ui.log_follow);
    if !app.core.logs.listed {
        loading(ui, "Looking for logs...");
        return;
    }
    let sources = app.core.logs.sources.clone();
    if sources.is_empty() {
        card(ui, |ui| note(ui, "No logs yet: they appear under /tmp/pimax-vr-session once desk or VR mode has run."));
        return;
    }
    if app.ui.log_source >= sources.len() {
        app.ui.log_source = 0;
    }
    card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("logsrc")
                .width(360.0)
                .selected_text(sources[app.ui.log_source].0.clone())
                .show_ui(ui, |ui| {
                    for (i, (label, _)) in sources.iter().enumerate() {
                        ui.selectable_value(&mut app.ui.log_source, i, label);
                    }
                });
            ui.label("Filter");
            ui.add(egui::TextEdit::singleline(&mut app.ui.log_filter).desired_width(180.0).hint_text("6DoF, Xid, ERROR ..."));
            ui.checkbox(&mut app.ui.log_follow, "Follow");
            if ui.button("Copy path").clicked() {
                ui.ctx().copy_text(sources[app.ui.log_source].1.display().to_string());
            }
        });
        let path = sources[app.ui.log_source].1.clone();
        let filter = app.ui.log_filter.to_lowercase();
        let Some(all) = app.core.logs.tail.as_ref().filter(|(p, _)| *p == path).map(|(_, l)| l) else {
            ui.horizontal(|ui| {
                spinner(ui);
                ui.label(RichText::new(format!("Reading {}...", path.display())).color(pal(ui).muted));
            });
            return;
        };
        let lines: Vec<String> = all.iter().filter(|x| filter.is_empty() || x.to_lowercase().contains(&filter)).cloned().collect();
        ui.label(RichText::new(format!("{} ({} lines shown)", path.display(), lines.len())).small().color(pal(ui).muted));
        let p = pal(ui);
        let row_h = ui.text_style_height(&egui::TextStyle::Monospace).max(14.0);
        egui::ScrollArea::both()
            .id_salt("logview")
            .max_height(520.0)
            .stick_to_bottom(app.ui.log_follow)
            .auto_shrink([false, false])
            .show_rows(ui, row_h, lines.len(), |ui, range| {
                for l in &lines[range] {
                    let c = if l.contains("ERROR")
                        || l.contains("Xid")
                        || l.contains("6DoF: lost")
                        || l.contains("optical fix lost")
                        || l.contains("built without -DPIMAX_CAM=ON")
                        || l.contains("Idling display engine")
                    {
                        p.fail
                    } else if l.contains("WARN") {
                        p.warn
                    } else if l.contains("6DoF: tracking") || l.contains("pipeline up") {
                        p.ok
                    } else {
                        ui.visuals().text_color()
                    };
                    ui.add(egui::Label::new(RichText::new(l).monospace().size(12.0).color(c)).extend());
                }
            });
    });
}

fn table_view(ui: &mut Ui, app: &mut PimaxApp, id: &str, t: Option<Table>, source: &str) {
    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label("Filter");
            ui.add(egui::TextEdit::singleline(&mut app.ui.history_filter).desired_width(240.0).hint_text("freeze, eye, Xid, 6DoF ..."));
            ui.label(RichText::new(source).small().color(pal(ui).muted));
        });
        match t {
            Some(t) => grid(ui, id, &t, &app.ui.history_filter.clone()),
            None => note(ui, &format!("{source} not found (docs/ next to the repository)")),
        }
    });
}

/// A table laid out row by row with fixed column widths: each row is as tall as its tallest wrapped cell in the
/// same frame (egui's Grid sizes rows from the previous frame, which overlaps multi-line cells).
fn grid(ui: &mut Ui, id: &str, t: &Table, filter: &str) {
    let rows = t.filter(filter);
    let n = t.header.len().max(1);
    let gap = 10.0;
    let total = (ui.available_width() - gap * n as f32).max(200.0);
    let weights: Vec<f32> = (0..n)
        .map(|c| {
            let longest = rows
                .iter()
                .map(|r| r.get(c).map_or(0, |s| s.chars().count()))
                .chain(std::iter::once(t.header.get(c).map_or(0, |h| h.chars().count())))
                .max()
                .unwrap_or(1);
            (longest as f32).clamp(8.0, 70.0)
        })
        .collect();
    let sum: f32 = weights.iter().sum();
    let widths: Vec<f32> = weights.iter().map(|w| (total * w / sum).max(48.0)).collect();
    let row = |ui: &mut Ui, cells: &[String], strong: bool| {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (i, w) in widths.iter().enumerate() {
                let text = cells.get(i).cloned().unwrap_or_default();
                ui.allocate_ui_with_layout(egui::vec2(*w, 0.0), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                    ui.set_width(*w);
                    let rt = if strong { RichText::new(text).strong() } else { RichText::new(text) };
                    ui.add(egui::Label::new(rt).wrap());
                });
            }
        });
    };
    ui.push_id(id, |ui| {
        row(ui, &t.header, true);
        ui.separator();
        for r in rows {
            row(ui, r, false);
            ui.separator();
        }
    });
}
