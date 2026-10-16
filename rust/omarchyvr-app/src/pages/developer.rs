//! Developer (developer mode): logs, crash reports and the problem report, what Start runs, the session details, the
//! auto mode daemon and the VR runtime switch.
use crate::app::PimaxApp;
use crate::pages::{desk, home, two_columns};
use crate::theme::{self, icon};
use crate::widgets::{self, chip, note, pill, section, switch_row};
use eframe::egui::{self, Id, RichText, Ui};
use omarchyvr_gui_core::app::Page;
use omarchyvr_gui_core::auto::AutoCmd;
use omarchyvr_gui_core::launcher::Preset;
use omarchyvr_gui_core::procs::fmt_age;

/// Draw the page.
pub fn show(ui: &mut Ui, app: &mut PimaxApp) {
    let crashes = app.core.crash_count();
    section(ui, "Developer", None, |ui| {
        ui.horizontal_wrapped(|ui| {
            if widgets::ibutton(ui, icon::FILE_TEXT, "Logs", true).clicked() {
                app.core.page = Page::Logs;
            }
            if widgets::ibutton(ui, icon::WRENCH, "Setup checklist", true).clicked() {
                app.core.page = Page::Setup;
            }
            let label = if crashes == 0 { "Crash reports".to_string() } else { format!("Crash reports ({crashes})") };
            if widgets::ibutton(ui, icon::FOLDER_OPEN, &label, true).clicked() {
                let d = omarchyvr_gui_core::crash::crashes_dir(&app.core.paths.state_dir);
                app.core.open_folder(&d);
            }
            if widgets::ibutton(ui, icon::BUG, "Report a problem...", true).clicked() {
                app.ui.report_open = true;
            }
        });
        note(ui, &format!(
            "omarchyvr {}. App log: {}",
            omarchyvr_gui_core::crash::version_line(),
            omarchyvr_gui_core::applog::log_file(&app.core.paths.state_dir).display()
        ));
        egui::CollapsingHeader::new("What Start runs").id_salt("plan-dev").show(ui, |ui| desk::plan_body(ui, app));
    });
    section(ui, "Session", None, |ui| desk::status(ui, app));
    two_columns(ui, 380.0, |i, ui| {
        if i == 0 {
            app.core.want_auto();
            section(ui, "Auto mode", Some("omarchyvr-autod starts the session when you put the headset on and lets it sleep when you leave; dry run until dry_run = false in ~/.config/omarchyvr/auto.toml."), |ui| auto_body(ui, app));
        } else {
            runtime(ui, app);
        }
    });
}

/// The OpenVR runtime of Steam's games: xrizer + Monado or SteamVR, the Steam launch options.
fn runtime(ui: &mut Ui, app: &mut PimaxApp) {
    section(ui, "VR runtime", None, |ui| {
        let o = app.core.facts.openvr;
        let (text, c) = match (o.exists, o.xrizer, o.read_only) {
            (false, _, _) => ("No OpenVR runtime file: Steam games cannot find a runtime.", theme::pal(ui).warn),
            (true, true, true) => ("Steam's OpenVR games go to Monado through xrizer (the runtime file is read-only).", theme::pal(ui).ok),
            (true, true, false) => ("xrizer + Monado; the runtime file is writable, so a SteamVR install may rewrite it (LESSONS row 20).", theme::pal(ui).warn),
            (true, false, _) => ("SteamVR is the OpenVR runtime: Steam games will not reach Monado.", theme::pal(ui).info),
        };
        widgets::status(ui, text, c);
        ui.horizontal_wrapped(|ui| {
            let busy = app.core.busy();
            if widgets::tbutton(ui, "Use xrizer + Monado", !busy).clicked() {
                app.core.runtime("xrizer");
            }
            if widgets::tbutton(ui, "Restore SteamVR", !busy).clicked() {
                app.core.runtime("steamvr");
            }
            if widgets::tbutton(ui, "Show status", !busy).clicked() {
                app.core.runtime("status");
            }
        });
        ui.horizontal(|ui| {
            widgets::caption(ui, "Steam launch options");
            widgets::help(ui, "Every Steam VR game needs this line in its Launch Options (Steam > game > Properties > General), once: without it Steam's container does not see Monado and the game runs flat. Then start VR here and press Play in Steam; SteamVR mode (no -openxr) is the default.");
        });
        widgets::code_box(ui, &[app.core.steam_options()]);
        ui.horizontal_wrapped(|ui| {
            chip(ui, "Proton games", theme::pal(ui).warn);
            widgets::row_tag(ui, Some(27));
        });
    });
}

/// The auto mode daemon: its state, dry run or live, pause, mode, runtime, preset, start and sleep now.
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
            widgets::status(ui, if e == "not running" { "Not running." } else { "The daemon did not answer." }, p.dim);
            if e != "not running" {
                widgets::status(ui, &e, p.warn);
            }
            let busy = app.core.busy();
            if app.core.facts.manual.autod_unit.is_some() && app.core.facts.manual.autod_build {
                ui.horizontal_wrapped(|ui| {
                    if widgets::ibutton(ui, icon::PLAY, "Start auto mode", !busy).on_hover_text("systemctl --user start omarchyvr-autod (dry run until auto.toml says otherwise)").clicked() {
                        app.core.autod_unit("start");
                    }
                    if widgets::ibutton(ui, icon::POWER, "Start at login", !busy).on_hover_text("systemctl --user enable omarchyvr-autod").clicked() {
                        app.core.autod_unit("enable");
                    }
                });
            } else {
                ui.horizontal(|ui| {
                    widgets::status(ui, "Not installed yet.", p.faint);
                    widgets::help(ui, "Setup > Install > Auto mode daemon builds it and installs its user unit and a dry-run config (nothing starts by itself).");
                });
                if widgets::ibutton(ui, icon::WRENCH, "Install it", true).clicked() {
                    app.core.act(&omarchyvr_gui_core::preflight::Action::Setup("auto-mode".into()));
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
            if switch_row(ui, &mut active, "Auto mode acts", Some("Off = paused: the states are still tracked."), true).changed() {
                app.core.auto_cmd(if active { AutoCmd::Resume } else { AutoCmd::Pause });
            }
            let mode = if s.mode == "game" { 1 } else { 0 };
            if let Some(i) = widgets::seg_row(ui, "Mode", None, &["Desk", "Game"], mode, true) {
                app.core.auto_cmd(AutoCmd::SetMode { mode: Some(if i == 1 { "game" } else { "desk" }.into()), runtime: None, preset: None });
            }
            let rt = if s.game_runtime == "steamvr" { 1 } else { 0 };
            if let Some(i) = widgets::seg_row(ui, "Game runtime", None, &["Monado + xrizer", "SteamVR (start it yourself)"], rt, true) {
                app.core.auto_cmd(AutoCmd::SetMode { mode: None, runtime: Some(if i == 1 { "steamvr" } else { "monado" }.into()), preset: None });
            }
            let names: Vec<&str> = Preset::ALL
                .iter()
                .map(|pr| match pr {
                    Preset::None => "None",
                    Preset::Cinema => "Cinema",
                    Preset::Work => "Work",
                    Preset::Assistant => "Assistant",
                })
                .collect();
            let cur = Preset::ALL.iter().position(|pr| s.preset == pr.name()).unwrap_or(usize::MAX);
            if let Some(i) = widgets::seg_row(ui, "Preset", Some("Changes here last until the daemon restarts; ~/.config/omarchyvr/auto.toml keeps them for good."), &names, cur, true) {
                app.core.auto_cmd(AutoCmd::SetMode { mode: None, runtime: None, preset: Some(Preset::ALL[i].name().into()) });
            }
            ui.horizontal_wrapped(|ui| {
                if widgets::ibutton(ui, icon::PLAY, "Start now", !s.paused).on_hover_text("wake: counts as wearing the headset").clicked() {
                    app.core.auto_cmd(AutoCmd::Wake);
                }
                if widgets::ibutton(ui, icon::POWER, "Sleep now", s.owned).on_hover_text("sleep: stops the session auto mode started (never a game)").clicked() {
                    app.core.auto_cmd(AutoCmd::Sleep);
                }
            });
            for side in ["left", "right"] {
                ui.horizontal_wrapped(|ui| {
                    widgets::caption(ui, if side == "left" { "Left controller" } else { "Right controller" });
                    home::pair_ui(ui, app, side);
                });
            }
        }
    }
    if !app.core.auto.last.is_empty() {
        widgets::status(ui, &app.core.auto.last, p.faint);
    }
}
