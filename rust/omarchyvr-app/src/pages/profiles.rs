//! Profiles & backups (the former omarchyvr-app page, now over `omarchyvr_gui_core::profiles`). Every button works on the
//! profile files in the background ([`omarchyvr_gui_core::app::AppCore::profiles_do`]).
use crate::app::PimaxApp;
use crate::widgets::{card, chip, note, pal};
use eframe::egui::{self, RichText, Ui};
use omarchyvr_config::{DiffLine, ProfileId, SerialCheck};
use omarchyvr_gui_core::profiles::DiffTarget;

enum Click {
    Import,
    Reload,
    Select(ProfileId),
    Activate,
    Backup,
    DiffLegacy,
    DiffBackup(usize),
    Restore,
}

/// Draw the page.
pub fn show(ui: &mut Ui, app: &mut PimaxApp) {
    let mut click = None;
    draw(ui, app, &mut click);
    if let Some(c) = click {
        match c {
            Click::Import => app.core.profiles_do(|p| p.import_legacy()),
            Click::Reload => app.core.profiles_do(|p| p.reload()),
            Click::Select(id) => app.core.profiles_do(move |p| p.select(id)),
            Click::Activate => app.core.profiles_do(|p| p.activate_selected()),
            Click::Backup => app.core.profiles_do(|p| p.backup_selected()),
            Click::DiffLegacy => app.core.profiles_do(|p| p.compute_diff(DiffTarget::Legacy)),
            Click::DiffBackup(i) => app.core.profiles_do(move |p| {
                p.backup_sel = Some(i);
                p.compute_diff(DiffTarget::Backup(i));
            }),
            Click::Restore => app.core.profiles_do(|p| p.restore_selected()),
        }
    }
}

fn draw(ui: &mut Ui, app: &mut PimaxApp, click: &mut Option<Click>) {
    let m = &mut app.core.profiles;
    let p = pal(ui);
    crate::widgets::section(ui, "Profiles & backups", Some("Per-headset lens.conf, eye correction and camera overlay, with a backup before every write."), |ui| {
        note(ui, &format!("Connected headset serial: {}", m.serial.as_deref().unwrap_or("none")));
        ui.horizontal(|ui| {
            if crate::widgets::tbutton(ui, "Import current ~/.config/omarchyvr files", true).clicked() {
                *click = Some(Click::Import);
            }
            if crate::widgets::tbutton(ui, "Reload", true).clicked() {
                *click = Some(Click::Reload);
            }
        });
        if !m.message.is_empty() {
            ui.label(RichText::new(&m.message).color(p.muted));
        }
    });
    if m.rows.is_empty() {
        card(ui, |ui| note(ui, "No profiles yet. Import creates <serial>/default from lens.conf, eye-correction.toml and cam-calib-overlay.json; the old files stay in place."));
        return;
    }
    let rows = m.rows.clone();
    card(ui, |ui| {
        for r in rows {
            ui.horizontal(|ui| {
                if ui.selectable_label(m.selected.as_ref() == Some(&r.id), r.id.to_string()).clicked() {
                    *click = Some(Click::Select(r.id.clone()));
                }
                if r.active {
                    chip(ui, "active", p.ok);
                }
                match r.serial {
                    SerialCheck::Match => {}
                    SerialCheck::Mismatch => chip(ui, "other headset", p.warn),
                    SerialCheck::Unknown => chip(ui, "headset not connected", p.muted),
                }
            });
        }
    });
    let Some(sel) = m.selected.clone() else { return };
    card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            if m.rows.iter().any(|r| r.id == sel && r.serial == SerialCheck::Mismatch) {
                ui.checkbox(&mut m.confirm_mismatch, "I know this profile is for another headset");
            }
            ui.checkbox(&mut m.link_legacy, "also link ~/.config/omarchyvr/lens.conf");
            if ui.button("Activate").clicked() {
                *click = Some(Click::Activate);
            }
            if ui.button("Back up now").clicked() {
                *click = Some(Click::Backup);
            }
            if ui.button("Diff with legacy files").clicked() {
                *click = Some(Click::DiffLegacy);
            }
        });
        ui.label(format!("Backups of {} ({}):", sel.serial, m.backups.len()));
        egui::ScrollArea::vertical().id_salt("bk").max_height(160.0).show(ui, |ui| {
            let names: Vec<(usize, String)> = m.backups.iter().enumerate().map(|(i, b)| (i, format!("{}   [{}]", b.name, b.files.join(" ")))).collect();
            for (i, label) in names {
                if ui.selectable_label(m.backup_sel == Some(i), label).clicked() {
                    *click = Some(Click::DiffBackup(i));
                }
            }
        });
        ui.horizontal(|ui| {
            if ui.add_enabled(m.backup_sel.is_some(), egui::Button::new("Restore selected backup")).clicked() {
                *click = Some(Click::Restore);
            }
            note(ui, "(diff shows backup -> current; restoring backs up the current state first)");
        });
        for (file, lines) in &m.diff {
            ui.label(RichText::new(file).strong());
            for l in lines {
                match l {
                    DiffLine::Same(_) => continue,
                    DiffLine::Removed(t) => ui.label(RichText::new(format!("- {t}")).monospace().color(p.fail)),
                    DiffLine::Added(t) => ui.label(RichText::new(format!("+ {t}")).monospace().color(p.ok)),
                };
            }
        }
    });
}
