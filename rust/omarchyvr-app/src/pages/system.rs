//! System: omarchyvrd's view of the stack (docs/PIMAXD.md). One row per component with its status dot and switch, Fix
//! for what needs a look, the recent events, the safe-mode switch, the GPU banner (acknowledge) and the reboot banner.
//! Everything that stops or restarts something asks first. The data is read in the background
//! (`omarchyvr_gui_core::system`).
use crate::app::PimaxApp;
use crate::theme::{self, icon};
use crate::widgets::{self, caption, card, chip, inset, note, pal, spinner, title};
use eframe::egui::{self, Color32, RichText, Ui};
use omarchyvr_gui_core::system::{Component, Dot, Snapshot, Source};

fn dot_color(ui: &Ui, d: Dot) -> Color32 {
    let p = theme::pal(ui);
    match d {
        Dot::Ok => p.ok,
        Dot::Degraded => p.warn,
        Dot::Down => p.fail,
        Dot::Off => p.faint,
        Dot::Unknown => p.info,
    }
}

fn overall_color(ui: &Ui, o: &str) -> Color32 {
    let p = theme::pal(ui);
    match o {
        "ok" => p.ok,
        "degraded" => p.warn,
        "down" | "needs-reboot" => p.fail,
        "needs-you" => p.warn,
        _ => p.dim,
    }
}

/// The sections of the component table (its kinds, in order).
const SECTIONS: [(&str, &str); 6] = [("device", "Devices"), ("process", "Processes"), ("feature", "Features"), ("setting", "Settings"), ("job", "Jobs"), ("", "Host")];

/// Draw the page.
pub fn show(ui: &mut Ui, app: &mut PimaxApp) {
    app.core.want_system();
    let snap = app.core.system.snap.clone();
    let source = app.core.system.source.clone();
    let live = source == Some(Source::Socket);
    header(ui, app, snap.as_ref(), source.as_ref(), live);
    let Some(s) = snap else { return };
    if s.needs_reboot() {
        reboot_banner(ui, app, &s, live);
    } else if s.gpu_caution() {
        gpu_banner(ui, app, &s, live);
    }
    session(ui, app, &s, live);
    components(ui, app, &s, live);
    events(ui, app);
}

fn header(ui: &mut Ui, app: &mut PimaxApp, s: Option<&Snapshot>, source: Option<&Source>, live: bool) {
    card(ui, |ui| {
        let p = theme::pal(ui);
        ui.horizontal_wrapped(|ui| {
            title(ui, "System");
            widgets::help(ui, "omarchyvrd watches the headset, the GPU and every part of the session, and repairs what breaks within its budgets. Observe only: it says what it would do and runs nothing (until [restart] enabled = true in ~/.config/omarchyvr/omarchyvrd.toml).");
            match source {
                Some(src) => chip(ui, &src.label(), if live { pal(ui).ok } else { pal(ui).muted }),
                None => {
                    spinner(ui);
                }
            }
            if let Some(s) = s {
                chip(ui, &s.overall, overall_color(ui, &s.overall));
                if s.observe_only {
                    chip(ui, "observe only", pal(ui).info);
                }
            }
            if app.core.demo {
                chip(ui, "demo data", pal(ui).info);
            }
        });
        match (s, source) {
            (None, Some(Source::None)) => {
                ui.label(RichText::new("omarchyvrd not running. docs/PIMAXD.md says how to run it (packaging/systemd/omarchyvrd.service).").color(p.warn_text));
                if let Some(e) = &app.core.system.error {
                    ui.label(RichText::new(e).size(11.5).color(p.faint));
                }
            }
            (Some(_), Some(Source::Health)) => {
                ui.label(RichText::new("omarchyvrd not running: this is a one-shot read-only probe (omarchyvr-health). The switches need omarchyvrd.").size(12.0).color(p.warn_text));
            }
            (Some(_), Some(Source::Mirror(_))) => {
                ui.label(RichText::new("From the state file omarchyvrd writes: the socket did not answer, so the switches wait for it.").size(12.0).color(p.warn_text));
            }
            _ => {}
        }
        if let Some(s) = s {
            ui.horizontal_wrapped(|ui| {
                let mut safe = s.safe_mode;
                let r = widgets::toggle(ui, &mut safe, "Safe mode", live);
                let r = if live { r.on_hover_text("Nothing runs automatically (~/.config/omarchyvr/safe-mode)") } else { r.on_disabled_hover_text("Needs omarchyvrd running") };
                if r.changed() {
                    app.core.system_safe_mode(safe);
                }
                if let Some(why) = &s.safe_reason {
                    ui.label(RichText::new(format!("omarchyvrd went into safe mode: {why}")).size(12.0).color(p.warn_text));
                    if widgets::ismall(ui, icon::WRENCH, "Fix omarchyvrd").on_hover_text("Leave the automatic safe mode (fix omarchyvrd)").clicked() {
                        app.core.system_fix("omarchyvrd");
                    }
                }
            });
            if let Some(a) = &s.next_action {
                ui.label(RichText::new(format!("Next: {} {} ({}): {}", a.what, a.id, a.level, a.why)).size(12.0).color(p.dim));
            }
            if let Some(b) = &s.busy {
                ui.label(RichText::new(format!("Busy: {b}")).size(12.0).color(p.info));
            }
        }
        if !app.core.system.reply.is_empty() {
            ui.label(RichText::new(&app.core.system.reply).size(11.5).color(p.faint));
        }
    });
}

fn evidence(ui: &mut Ui, s: &Snapshot) {
    if s.gpu_evidence.is_empty() {
        return;
    }
    egui::CollapsingHeader::new(RichText::new("Kernel lines").small()).id_salt("gpu-evidence").show(ui, |ui| {
        for l in &s.gpu_evidence {
            ui.add(egui::Label::new(RichText::new(l).monospace().size(11.0).color(theme::pal(ui).dim)).wrap());
        }
    });
}

fn reboot_banner(ui: &mut Ui, app: &mut PimaxApp, s: &Snapshot, live: bool) {
    banner(ui, theme::pal(ui).fail, |ui| {
        ui.label(RichText::new(format!("{} Reboot needed", icon::WARNING)).strong().size(16.0).color(theme::pal(ui).fail));
        note(ui, "The GPU logged errors this boot (an Xid, or the driver does not match the kernel module): nothing starts automatically until a reboot. The app never reboots for you. If the only cause was the VR runtime being killed or hung (no GPU error), it drops to a warning by itself once a VR session has run cleanly for 5 minutes, and then you can acknowledge it here.");
        evidence(ui, s);
        let r = ui.add_enabled(live, egui::Button::new("Start a session anyway, once..."));
        if r.on_hover_text("set session=on with force: passes the reboot gate once (confirmed first)").clicked() {
            app.core.system_force_start();
        }
    });
}

fn gpu_banner(ui: &mut Ui, app: &mut PimaxApp, s: &Snapshot, live: bool) {
    banner(ui, theme::pal(ui).warn, |ui| {
        ui.label(RichText::new(format!("{} GPU warning", icon::WARNING)).strong().size(16.0).color(theme::pal(ui).warn));
        note(ui, "The GPU logged recoverable errors this boot (for example a WayVR Xid 31, or a VR runtime that was killed and then ran cleanly for 5 minutes). Sessions start again, and recording can use the GPU encoder, once you acknowledge them; a reboot clears them.");
        evidence(ui, s);
        let r = widgets::primary(ui, "Acknowledge", live);
        let r = if live { r.on_hover_text("ack-gpu (confirmed first)") } else { r.on_disabled_hover_text("Needs omarchyvrd running") };
        if r.clicked() {
            app.core.system_ack_gpu();
        }
    });
}

fn banner(ui: &mut Ui, color: Color32, add: impl FnOnce(&mut Ui)) {
    let p = theme::pal(ui);
    egui::Frame::new()
        .fill(theme::mix(p.card, color, 0.10))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.7)))
        .corner_radius(egui::CornerRadius::same(theme::R_CARD))
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

fn session(ui: &mut Ui, app: &mut PimaxApp, s: &Snapshot, live: bool) {
    card(ui, |ui| {
        let p = theme::pal(ui);
        title(ui, "Session");
        ui.horizontal_wrapped(|ui| {
            let mut on = s.intent_session == "on";
            let r = widgets::toggle(ui, &mut on, &format!("Session ({})", s.intent_mode), live);
            let r = if live { r.on_hover_text("omarchyvrd keeps the session on or off (off stops it: confirmed first)") } else { r.on_disabled_hover_text("Needs omarchyvrd running") };
            if r.changed() {
                app.core.system_session(on);
            }
            ui.label(RichText::new(format!("wanted {} (by {})", s.intent_session, if s.intent_by.is_empty() { "-" } else { &s.intent_by })).size(12.0).color(p.dim));
        });
        let running = match &s.session_mode {
            Some(m) => format!("Running: {m} mode, launcher {}{}", if s.launcher.is_empty() { "idle" } else { &s.launcher }, if s.watchdog { ", the launcher's watchdog runs" } else { "" }),
            None => "No session runs.".into(),
        };
        ui.label(RichText::new(running).size(12.5).color(p.dim));
    });
}

fn row(ui: &mut Ui, app: &mut PimaxApp, c: &Component, live: bool) {
    let p = theme::pal(ui);
    ui.horizontal(|ui| {
        widgets::dot(ui, dot_color(ui, c.dot()), 5.0);
        ui.label(RichText::new(if c.label.is_empty() { &c.id } else { &c.label }).strong());
        ui.label(RichText::new(&c.id).size(10.5).color(p.faint));
        ui.label(RichText::new(&c.health).size(12.0).color(dot_color(ui, c.dot())));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if c.toggle != "none" {
                let mut on = c.wanted;
                let can = live && c.live_toggle();
                let r = widgets::toggle(ui, &mut on, "", can);
                r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, can, c.wanted, &c.id));
                let r = if !live {
                    r.on_disabled_hover_text("Needs omarchyvrd running")
                } else if let Some(p) = c.toggle.strip_prefix("restart:") {
                    r.on_disabled_hover_text(format!("Needs a restart of {p}: applies at the next start (Restart service)"))
                } else {
                    r.on_hover_text(if c.wanted { "Turn off for this session (confirmed first)" } else { "Turn on for this session" })
                };
                if r.changed() {
                    app.core.system_toggle(&c.id, on);
                }
            }
            if c.finding() {
                let r = ui.add_enabled(live, egui::Button::new(format!("{} Fix", icon::WRENCH)));
                r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, live, format!("Fix {}", c.id)));
                if r.on_hover_text("omarchyvrd forgets the failures and tries its next repair once (confirmed first)").clicked() {
                    app.core.system_fix(&c.id);
                }
            }
        });
    });
    let mut bits: Vec<String> = Vec::new();
    if let Some(w) = &c.why {
        bits.push(if app.core.system.snap.as_ref().is_some_and(|s| s.component(w).is_some()) { format!("waits for {w}") } else { w.clone() });
    }
    if let Some(d) = &c.detail {
        bits.push(d.clone());
    }
    if let Some(r) = &c.restarts {
        if r.used > 0 || r.breaker != "closed" {
            bits.push(format!("restarts {}{}, breaker {}", r.used, r.budget.as_deref().map(|b| format!(" of {b}")).unwrap_or_default(), r.breaker));
        }
    }
    if let Some(e) = &c.last_exit {
        bits.push(format!("last exit: {} ({})", e.class, e.how));
    }
    if !bits.is_empty() {
        ui.indent(("sys-detail", &c.id), |ui| {
            ui.add(egui::Label::new(RichText::new(bits.join("  |  ")).size(11.5).color(p.dim)).wrap());
        });
    }
}

fn components(ui: &mut Ui, app: &mut PimaxApp, s: &Snapshot, live: bool) {
    card(ui, |ui| {
        title(ui, "Components");
        let findings = s.components.iter().filter(|c| c.finding()).count();
        if findings > 0 {
            ui.label(RichText::new(format!("{findings} to look at")).size(12.0).color(theme::pal(ui).warn));
        }
        for (kind, name) in SECTIONS {
            let list: Vec<&Component> = if kind.is_empty() {
                s.components.iter().filter(|c| !SECTIONS.iter().any(|(k, _)| !k.is_empty() && c.kind == *k)).collect()
            } else {
                s.components.iter().filter(|c| c.kind == kind).collect()
            };
            if list.is_empty() {
                continue;
            }
            ui.add_space(4.0);
            caption(ui, name);
            inset(ui, |ui| {
                for c in list {
                    row(ui, app, c, live);
                    ui.add_space(2.0);
                }
            });
        }
    });
}

fn events(ui: &mut Ui, app: &mut PimaxApp) {
    card(ui, |ui| {
        let p = theme::pal(ui);
        title(ui, "Recent events");
        if app.core.system.events.is_empty() {
            note(ui, "None yet (they arrive while this page is open).");
        }
        for e in app.core.system.events.iter().take(25) {
            let c = match e.event.as_str() {
                "critical" => p.fail,
                "notice" | "would" => p.warn,
                "step" | "done" => p.ok,
                "health" => p.info,
                _ => p.dim,
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{:>7.1}", e.t)).monospace().size(11.0).color(p.faint));
                ui.label(RichText::new(&e.event).size(11.0).color(c));
                if let Some(id) = &e.id {
                    ui.label(RichText::new(id).size(11.0).color(p.text));
                }
                ui.add(egui::Label::new(RichText::new(&e.text).size(11.5).color(p.dim)).truncate());
            });
        }
    });
}
