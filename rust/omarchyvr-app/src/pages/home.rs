//! Home, the dashboard: the headset card (line art, state, refresh rate, Play / Work / Stop, the start verdict), the
//! controllers card (pair, unpair, vibrate), and three small cards: quick settings, tracking, sound. Details live on
//! the other pages; explanations are behind the "?" marks and on the Help page.
use crate::app::PimaxApp;
use crate::art::{self, Art};
use crate::pages::{desk, editable};
use crate::theme::{self, caps, icon};
use crate::widgets::{self, chip, pill, switch, switch_row, Kind};
use eframe::egui::{self, Id, RichText, Ui};
use omarchyvr_gui_core::app::{DeskPhase, Page};
use omarchyvr_gui_core::devices::{devices, Battery, Dev, DeviceView};
use omarchyvr_gui_core::headsets;
use omarchyvr_gui_core::play::CapState;
use omarchyvr_gui_core::preflight::{Gate, Preflight};
use omarchyvr_gui_core::procs::fmt_age;

/// Draw the page.
pub fn show(ui: &mut Ui, app: &mut PimaxApp) {
    let devs = devices(&app.core.facts, &app.core.live, &app.core.toggles, app.core.demo);
    widgets::card_row(ui, "home-top", &[0.57, 0.43], 330.0, |i, ui| {
        if i == 0 {
            headset_card(ui, app, &devs.headset);
        } else {
            controllers_card(ui, app, &devs.left, &devs.right);
        }
    });
    let w = ui.available_width();
    if w >= 860.0 {
        widgets::card_row(ui, "home-cards", &[1.0, 1.0, 1.0], 220.0, |i, ui| match i {
            0 => quick_settings(ui, app),
            1 => tracking_card(ui, app, &devs.tracking),
            _ => sound_card(ui, app),
        });
    } else {
        widgets::card_row(ui, "home-cards-2", &[1.0, 1.0], 220.0, |i, ui| match i {
            0 => quick_settings(ui, app),
            _ => tracking_card(ui, app, &devs.tracking),
        });
        widgets::card(ui, |ui| sound_card(ui, app));
    }
}

/// The headset: name and state, the line art, a few facts, the model picker, then the session (Play / Work / Stop).
fn headset_card(ui: &mut Ui, app: &mut PimaxApp, v: &DeviceView) {
    let p = theme::pal(ui);
    let detected = headsets::connected(&app.core.facts).map(|m| m.id().to_string());
    let chosen: String = app.ui.model.map(String::from).or(detected).unwrap_or_else(|| headsets::default_module().id().to_string());
    let model = headsets::by_id(&chosen).unwrap_or_else(headsets::default_module);
    let mock = model.is_mock();
    let name = model.descriptor().name.clone();
    let (label, state) = if mock { ("Not supported yet".to_string(), Dev::Off) } else { (v.label.clone(), v.state) };
    let mut pick: Option<&'static str> = None;
    widgets::title_row(ui, &name, None, |ui| {
        pill(ui, Id::new("headset-pill"), &label, widgets::dev_color(&p, state));
    });
    ui.add_space(6.0);
    let inner = ui.available_width();
    let wide = inner >= 340.0;
    let art_w = if wide { (inner * 0.42).clamp(130.0, 280.0) } else { inner.min(240.0) };
    let key_w = if inner < 420.0 { 70.0 } else { 86.0 };
    let art_h = art_w / Art::Headset.aspect();
    let accent = theme::fade(ui, Id::new("headset-art"), if state == Dev::Off { p.faint } else { p.accent });
    let facts = |ui: &mut Ui, app: &mut PimaxApp, pick: &mut Option<&'static str>| {
        ui.spacing_mut().item_spacing.y = 4.0;
        if mock {
            for c in headsets::caps(model.descriptor()) {
                widgets::kv_row(ui, &c.name, key_w, |ui| {
                    ui.label(RichText::new(headsets::support_label(c.support, true)).size(12.0).color(p.faint));
                });
            }
        } else {
            let ed = editable(app);
            widgets::kv_row(ui, "Refresh", key_w, |ui| {
                let cur = usize::from(app.core.toggles.hz90);
                if let Some(i) = widgets::seg(ui, "home-hz", &["72 Hz", "90 Hz"], cur, ed) {
                    app.core.toggles.hz90 = i == 1;
                }
            });
            let pct = app.core.state.scale_pct;
            let q = omarchyvr_gui_core::quality::preset_of(pct).map(str::to_string).unwrap_or_else(|| format!("{pct} %"));
            widgets::kv_row(ui, "Render", key_w, |ui| {
                ui.label(RichText::new(q).size(12.5).color(p.text)).on_hover_text(omarchyvr_gui_core::quality::describe(pct));
            });
            let ipd = app.core.facts.ipd_mm.map(|m| format!("{m:.1} mm")).unwrap_or_else(|| "not set".into());
            widgets::kv_row(ui, "IPD", key_w, |ui| {
                ui.label(RichText::new(ipd).size(12.5).color(p.text));
            });
            if !v.detail.is_empty() {
                widgets::kv_row(ui, "Status", key_w, |ui| {
                    ui.add(egui::Label::new(RichText::new(&v.detail).size(12.5).color(p.dim)).truncate());
                });
            }
        }
        widgets::kv_row(ui, "Model", key_w, |ui| {
            egui::ComboBox::from_id_salt("headset-model").selected_text(RichText::new("Change").size(12.0)).width(110.0).show_ui(ui, |ui| {
                for m in headsets::all().iter() {
                    let id: &'static str = m.id();
                    let n = &m.descriptor().name;
                    let label = if m.is_mock() { format!("{n} (not supported yet)") } else { n.clone() };
                    if ui.selectable_label(id == chosen, label).clicked() {
                        *pick = Some(id);
                    }
                }
            });
        });
    };
    if wide {
        ui.horizontal_top(|ui| {
            art::show(ui, Art::Headset, egui::vec2(art_w, art_h.max(80.0)), accent, p.dim);
            ui.add_space(14.0);
            ui.vertical(|ui| facts(ui, app, &mut pick));
        });
    } else {
        ui.vertical_centered(|ui| art::show(ui, Art::Headset, egui::vec2(art_w, art_h), accent, p.dim));
        facts(ui, app, &mut pick);
    }
    if let Some(id) = pick {
        app.ui.model = Some(id);
    }
    if mock {
        return;
    }
    ui.add_space(8.0);
    session(ui, app);
    reset_state(ui, app);
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
        widgets::status(ui, &r.text, if r.ok { p.ok } else { p.warn });
    }
    if let Some(k) = app.core.restart_service_progress() {
        let secs = (k * omarchyvr_gui_core::reset::RESTART_SECS as f32) as u64;
        ui.add(egui::ProgressBar::new(k).desired_width(ui.available_width()).text(format!("Restarting the service: {secs} s")));
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    }
    if let Some(w) = app.core.watchdog_state() {
        ui.add(egui::Label::new(RichText::new(format!("{} {w}", icon::PULSE)).size(11.5).color(p.dim)).truncate());
    }
}

/// The readiness dots: headset, the Monado cap (with Fix), the OpenXR runtime, the last start. Short words; the
/// sentence is the tooltip.
fn readiness(ui: &mut Ui, app: &mut PimaxApp) {
    let p = theme::pal(ui);
    let r = app.core.readiness();
    let color = |ok: Option<bool>| match ok {
        Some(true) => p.ok,
        Some(false) => p.warn,
        None => p.faint,
    };
    let item = |ui: &mut Ui, ok: Option<bool>, word: &str, text: &str| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            widgets::dot(ui, color(ok), 3.0);
            ui.label(caps(word, 10.0, p.dim));
        })
        .response
        .on_hover_text(text);
        ui.add_space(6.0);
    };
    let mut fix = false;
    ui.horizontal_wrapped(|ui| {
        item(ui, r.headset.0, "Headset", &r.headset.1);
        match &r.cap {
            CapState::Ok(s) => item(ui, Some(true), "Monado cap", &format!("{} has cap_sys_nice", s.display())),
            CapState::Missing(s) => {
                widgets::dot(ui, p.fail, 3.0);
                ui.label(caps("Monado cap missing", 10.0, p.fail))
                    .on_hover_text(format!("{} lost cap_sys_nice (every rebuild drops it): frames can stutter. Fix asks for your password once.", s.display()));
                if widgets::ghost(ui, icon::WRENCH, "Fix", !app.core.busy())
                    .on_hover_text("pkexec setcap cap_sys_nice+ep on this monado-service (a password window), then checks again")
                    .clicked()
                {
                    fix = true;
                }
                ui.add_space(6.0);
            }
            CapState::Unknown(_) => item(ui, None, "Monado cap", "Monado cap: checking"),
        }
        item(ui, r.runtime.0, "Runtime", &r.runtime.1);
        item(ui, r.last.0, "Last start", &r.last.1);
    });
    if let Some(m) = &app.core.play.last_msg {
        ui.add(egui::Label::new(RichText::new(m).size(11.5).color(p.faint)).truncate());
    }
    if fix {
        app.core.fix_cap();
    }
}

/// The session: Play (games) and Work (the desk); the running side's button stops it. Then the verdict of the
/// preflight with its details, and what runs now.
fn session(ui: &mut Ui, app: &mut PimaxApp) {
    let p = theme::pal(ui);
    let phase = app.core.phase();
    let busy = app.core.busy();
    let w = ui.available_width();
    let half = ((w - 10.0) / 2.0).floor();
    // Games run with: SteamVR: Steam restarted or closed, a Steam that SteamVR started, SteamVR quit (docs/STEAMVR.md)
    if let Some(n) = &app.core.live.steam_note {
        widgets::status(ui, n, p.warn);
    }
    // an xrizer crash of the last game, and a listed game that runs under xrizer now (docs/GAME-MODE.md)
    if let Some(n) = app.core.game_watch.crash_note.clone() {
        widgets::status(ui, &n, p.warn);
        if ui.small_button("Got it").clicked() {
            app.core.game_watch.crash_note = None;
        }
    }
    if let Some(e) = app.core.game_needs_steamvr_now() {
        widgets::status(ui, &format!("{} crashes with xrizer. Switch Play to SteamVR for it (button below), or close the game.", e.name), p.warn);
        if ui.add_enabled(!busy, eframe::egui::Button::new("Switch Play to SteamVR for this game")).on_hover_text("Stops this Play session (by pid) and starts it again with SteamVR for this game. Then close the game if it is still open and start it again from Steam.").clicked() {
            app.core.switch_to_steamvr_for_game(&e.appid);
        }
    }
    match phase {
        DeskPhase::Running => {
            let vr = app.core.running_side() == Some(true);
            let hover = if vr {
                "Stops Play: the game session, SteamVR or Monado, the trackers (by pid; Monado SIGINT, SIGKILL only after 30 s)"
            } else {
                "Stops everything of Work: the screens, voice, the local AI, the desk's virtual displays; the sound goes back (by pid; Monado SIGINT, SIGKILL only after 30 s)"
            };
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                for (side_vr, label) in [(true, "Stop Play"), (false, "Stop Work")] {
                    if side_vr == vr {
                        if widgets::action(ui, icon::STOP, label, Kind::Danger, !busy, half).on_hover_text(hover).clicked() {
                            app.core.stop();
                        }
                    } else {
                        // the other side: one job that stops this side completely, then starts that one
                        let (other, ic, why) = if side_vr {
                            ("Switch to Play", icon::PLAY, "Stops Work completely first (screens, voice, the local AI: the graphics card is free for the game), then starts Play")
                        } else {
                            ("Switch to Work", icon::DESKTOP, "Stops Play first, then starts Work")
                        };
                        let r = widgets::action(ui, ic, other, Kind::Secondary, !busy, half).on_hover_text(why);
                        if r.clicked() {
                            if side_vr {
                                app.core.play();
                            } else {
                                app.core.work();
                            }
                        }
                    }
                }
            });
            running_lines(ui, app, vr);
        }
        DeskPhase::Foreign => {
            widgets::status(ui, "Another monado-service is running", p.fail);
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
            // the two main choices, side by side: Play (games) and Work (the desk)
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                if big_choice(ui, icon::GAME_CONTROLLER, "Play", "Games: SteamVR and OpenXR", can(&vpf, consent), half)
                    .on_hover_text(format!("Games: the Pimax services (Monado, 6DoF, the controllers and their tracking) for Steam and OpenXR games, and the whole graphics card for the game: no desk, no voice, no local AI.{first}"))
                    .clicked()
                {
                    app.core.play();
                }
                let setup = if app.core.work_setup_done() { "" } else { " The first time, two questions come first (voice assistant, speech)." };
                if big_choice(ui, icon::DESKTOP, "Work", "Desk: screens, voice, AI", can(&pf, consent), half)
                    .on_hover_text(format!("The WayVR desk: screens, passthrough, voice, the AI, captions, presets.{setup}{first}"))
                    .clicked()
                {
                    app.core.work();
                }
            });
            ui.add_space(10.0);
            super::roomsetup::tile(ui, app, w);
            if cap {
                widgets::status(ui, "Play and Work first restore the Monado cap: your password is asked once.", p.warn);
            }
            let last = if app.core.play_is_vr() { vpf } else { pf };
            verdict(ui, app, &last);
        }
        _ => {
            ui.horizontal(|ui| {
                widgets::spinner(ui);
                ui.label(RichText::new(app.core.job.as_ref().map(|j| j.label.clone()).unwrap_or_else(|| "Working...".into())).size(12.5).color(p.dim));
            });
            let _ = widgets::action(ui, icon::PLAY, "Working...", Kind::Secondary, false, w);
        }
    }
    readiness(ui, app);
    match &app.core.play.autostart_note {
        Some((ok, n)) => widgets::status(ui, n, if *ok { p.ok } else { p.warn }),
        None if app.core.play.autostart_pending => widgets::status(ui, "Start at login: waiting for the first check and the headset...", p.faint),
        None => {}
    }
}

/// What runs now: the side and its age, the render size, what game mode paused, the frame health.
fn running_lines(ui: &mut Ui, app: &mut PimaxApp, vr: bool) {
    let p = theme::pal(ui);
    let age = app.core.live.service.as_ref().and_then(|s| s.age).map(fmt_age).unwrap_or_default();
    ui.horizontal_wrapped(|ui| {
        widgets::dot(ui, p.ok, 3.0);
        ui.label(caps(&format!("{} {age}", if vr { "Play running" } else { "Work running" }), 10.5, p.ok));
        if let Some(sp) = app.core.live.scale_pct {
            ui.label(RichText::new(omarchyvr_gui_core::quality::describe(sp)).size(11.5).color(p.dim));
        }
    });
    if vr {
        // Play keeps the whole graphics card for the game: the read-only check, every 10 s
        app.core.want_vram();
        if let Some((_, r)) = &app.core.vram {
            let (ok, line) = r.line();
            let c = match ok {
                Some(true) => p.ok,
                Some(false) => p.warn,
                None => p.faint,
            };
            let others = r.others();
            let hover = if others.is_empty() { "nvidia-smi, read-only: no other program holds video memory".to_string() } else { format!("Also using video memory (not ours, never touched): {}", others.join(", ")) };
            ui.horizontal_wrapped(|ui| {
                widgets::dot(ui, c, 3.0);
                ui.add(egui::Label::new(RichText::new(line).size(11.5).color(if ok == Some(false) { p.warn } else { p.dim })).wrap()).on_hover_text(hover);
            });
        }
    }
    if !app.core.live.gm_paused.is_empty() {
        ui.label(RichText::new(format!("Stopped for Play: {}", app.core.live.gm_paused.join(", "))).size(11.5).color(p.dim)).on_hover_text("They come back with the next Work, not after Play.");
    }
    if let Some(ps) = &app.core.live.perf {
        let c = if ps.dropped_pct() > 1.0 { p.warn } else { p.dim };
        ui.label(RichText::new(ps.summary()).size(11.5).color(c)).on_hover_text(format!(
            "Dropped = the headset showed an image twice ({} of {} frames since Monado started). Separately, the compositor's \
             own commit ended after the predicted display time in {:.0} % of frames: not a dropped frame, but a sign of \
             compositor latency (Help > Game mode).",
            ps.skipped, ps.frames, ps.late_pct()
        ));
    }
}

/// The verdict pill with its reason, the consent box, the details.
fn verdict(ui: &mut Ui, app: &mut PimaxApp, pf: &Preflight) {
    let p = theme::pal(ui);
    let (word, text, color) = match &pf.gate {
        Gate::Ready => ("ready", String::new(), p.ok),
        Gate::NeedsConsent(r) => ("your OK", format!("Needs your OK: {}", r.join("; ")), p.warn),
        Gate::Blocked(r) => ("blocked", format!("Blocked: {}", r.join("; ")), p.fail),
    };
    ui.horizontal_wrapped(|ui| {
        pill(ui, Id::new("verdict"), word, color);
        if !text.is_empty() {
            ui.add(egui::Label::new(RichText::new(text).size(12.0).color(p.dim)).wrap());
        }
    });
    if matches!(pf.gate, Gate::NeedsConsent(_)) {
        ui.checkbox(&mut app.core.consent_force, RichText::new("Start anyway (only WayVR Xid 31 faults; adds --force)").size(12.5).color(p.warn_text))
            .on_hover_text("LESSONS row 9: per-app MMU faults the GPU recovers from. Any other Xid or a display engine error blocks the start.");
    }
    egui::CollapsingHeader::new(caps(&format!("Preflight: {} checks", pf.findings.len()), 10.5, p.dim))
        .id_salt("preflight-details")
        .show(ui, |ui| widgets::inset(ui, |ui| desk::banner_body(ui, app, pf)));
}

/// One of the two big choices of Home (Play, Work): an icon, the name, one line of what it is. Drawn with the theme's
/// colours, so it follows the app's look.
pub(crate) fn big_choice(ui: &mut Ui, ic: &str, name: &str, sub: &str, enabled: bool, width: f32) -> egui::Response {
    let p = theme::pal(ui);
    let h = 76.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, h), if enabled { egui::Sense::click() } else { egui::Sense::hover() });
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, name));
    let hover = theme::ease(ui, resp.id.with("hover"), enabled && resp.hovered(), theme::T_HOVER);
    let (fill, fg, dim, stroke) = if enabled {
        (theme::mix(p.accent, p.accent_hover, hover), egui::Color32::WHITE, egui::Color32::from_white_alpha(200), p.accent_hover)
    } else {
        (egui::Color32::TRANSPARENT, p.faint, p.faint, p.stroke)
    };
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect(rect, egui::CornerRadius::same(8), fill, egui::Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
        let icon_g = painter.layout_no_wrap(ic.to_string(), egui::FontId::proportional(26.0), fg);
        let ix = rect.left() + 16.0;
        painter.galley(egui::pos2(ix, rect.center().y - icon_g.size().y / 2.0), icon_g, fg);
        let tx = ix + 40.0;
        let title = painter.layout_no_wrap(name.to_uppercase(), egui::FontId::new(18.0, theme::heading(ui)), fg);
        let title_h = title.size().y;
        painter.galley(egui::pos2(tx, rect.center().y - title_h + 2.0), title, fg);
        let sub_g = painter.layout(sub.to_string(), egui::FontId::proportional(12.0), dim, (rect.right() - tx - 8.0).max(40.0));
        painter.galley(egui::pos2(tx, rect.center().y + 4.0), sub_g, dim);
    }
    resp
}

/// Both controllers: the line art, the state, the battery, Pair / Unpair / Vibrate.
fn controllers_card(ui: &mut Ui, app: &mut PimaxApp, left: &DeviceView, right: &DeviceView) {
    let p = theme::pal(ui);
    let up = controllers_up(app);
    widgets::title_row(ui, "Controllers", None, |ui| {
        pill(ui, Id::new("ctrl-count"), &format!("{up}/2 connected"), if up == 2 { p.ok } else if up == 1 { p.accent } else { p.faint });
    });
    ui.add_space(6.0);
    let one = |ui: &mut Ui, app: &mut PimaxApp, i: usize, v: &DeviceView| {
        let side = if i == 0 { "left" } else { "right" };
        let art = if i == 0 { Art::ControllerLeft } else { Art::ControllerRight };
        let accent = theme::fade(ui, Id::new(("ctrl-art", side)), if v.state == Dev::Off { p.faint } else { p.accent });
        ui.horizontal_top(|ui| {
            art::show(ui, art, egui::vec2(46.0, 74.0), accent, p.dim);
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                ui.label(caps(&v.name, 11.5, p.text).family(theme::heading(ui))).on_hover_text(&v.detail);
                ui.horizontal_wrapped(|ui| {
                    pill(ui, Id::new(("ctrl-pill", side)), &v.label, widgets::dev_color(&p, v.state));
                    battery(ui, v.battery);
                    in_hand(ui, app, i);
                });
                ui.horizontal_wrapped(|ui| pair_ui(ui, app, side));
            });
        });
    };
    if ui.available_width() >= 560.0 {
        // a wide card (a narrow window, where the cards are one under the other): the two side by side
        ui.columns(2, |c| {
            one(&mut c[0], app, 0, left);
            one(&mut c[1], app, 1, right);
        });
    } else {
        one(ui, app, 0, left);
        ui.add_space(2.0);
        ui.separator();
        ui.add_space(2.0);
        one(ui, app, 1, right);
    }
    ui.add_space(4.0);
    controllers_modes(ui, app);
}

/// In hand / put down (the running session's PIMAX_CTRL_INHAND status), next to the battery.
fn in_hand(ui: &mut Ui, app: &PimaxApp, i: usize) {
    let p = theme::pal(ui);
    let Some(st) = &app.core.live.ctrl_inhand else { return };
    let s = if i == 0 { &st.left } else { &st.right };
    chip(ui, s.words(), if s.shown() { p.ok } else { p.faint });
}

/// When the controllers show: in Work (default when picked up) and in games (default always).
fn controllers_modes(ui: &mut Ui, app: &mut PimaxApp) {
    use omarchyvr_config::ctrl_inhand::CtrlMode;
    let names: Vec<&str> = CtrlMode::ALL.iter().map(|m| m.label()).collect();
    let ok = app.core.facts.caps.controllers && !app.core.busy();
    let idx = |m: CtrlMode| CtrlMode::ALL.iter().position(|x| *x == m).unwrap_or(0);
    let work = idx(app.core.toggles.ctrl_work);
    if let Some(i) = widgets::seg_row(ui, "In Work", Some(HELP_CTRL_WORK), &names, work, ok) {
        app.core.set_controllers_mode(false, CtrlMode::ALL[i]);
    }
    let play = idx(app.core.toggles.ctrl_play);
    if let Some(i) = widgets::seg_row(ui, "In games", Some(HELP_CTRL_PLAY), &names, play, ok) {
        app.core.set_controllers_mode(true, CtrlMode::ALL[i]);
    }
}

const HELP_CTRL_WORK: &str = "When picked up (default): a controller lying on the desk is hidden, no model, no ray, no jumping. \
Pick it up or touch a button and it is back at once; 5 s still puts it away again. Always: shown all the time. Off: never. \
Voice: \"Max, controllers on\" (always) / \"Max, controllers off\", until the session ends.";
const HELP_CTRL_PLAY: &str = "Always (default): games see both controllers all the time, like on Windows. \
When picked up: hidden while they lie still (some games pause or warn without controllers).";

/// Controllers the running session has seen (0 when nothing runs: they connect through it).
pub(crate) fn controllers_up(app: &PimaxApp) -> usize {
    use omarchyvr_gui_core::devices::{SLOT_LEFT, SLOT_RIGHT};
    if app.core.live.service.is_none() {
        return 0;
    }
    [SLOT_LEFT, SLOT_RIGHT].iter().filter(|s| app.core.live.log.ctrl.slots_seen.contains(s)).count()
}

/// The battery, when the controller reports it (a faint mark when it does not).
fn battery(ui: &mut Ui, b: Option<Battery>) {
    let p = theme::pal(ui);
    match b {
        Some(Battery::Level(x)) => chip(ui, &format!("{x}%"), if x > 30 { p.dim } else { p.warn }),
        Some(Battery::Mock(x)) => {
            ui.label(caps(&format!("{} {x}%", icon::BATTERY_HIGH), 10.0, p.faint)).on_hover_text("Demo value (mock)");
        }
        Some(Battery::Unknown) => {
            ui.label(RichText::new(icon::BATTERY_EMPTY).size(13.0).color(p.faint)).on_hover_text("Battery unknown: Monado does not report it");
        }
        None => {}
    }
}

/// The switches used most, also on their own pages.
fn quick_settings(ui: &mut Ui, app: &mut PimaxApp) {
    widgets::title_row(ui, "Quick settings", None, |_| {});
    ui.add_space(4.0);
    let ed = editable(app);
    let caps = app.core.facts.caps.clone();
    let t = app.core.toggles;
    let busy = app.core.busy();
    switch_row(ui, &mut app.core.toggles.eye, "Eye tracking", Some("Visual only: a dot where you look in Work. Starts with every session; until you calibrate your own eyes it uses the built-in test calibration of the developer (rough aim). Details on the Tracking page."), ed && (caps.eye || caps.no_eye || t.eye));
    switch_row(ui, &mut app.core.toggles.passthrough, "Passthrough", Some("The room behind the screens in Work (camera view)."), ed && (caps.no_passthrough || !t.passthrough));
    subtitles_row(ui, app);
    let mut light = app.core.light_game();
    if switch_row(ui, &mut light, "Light game mode", Some("On (default): Play runs only what a game needs: Monado, head tracking (6DoF), the controllers and their tracking. No wrist readout or room search. Off: Play also runs the wrist readout and the room anchor. Either way Play never runs the local AI, voice or the desk: the graphics card is the game's. Applies to the next Play."), !busy).changed() {
        app.core.set_light_game(light);
    }
    let was_on = app.core.facts.eye_games;
    let mut eye_games = was_on;
    if switch_row(ui, &mut eye_games, "Eye tracking in games", Some("Off (default): Play leaves the eye tracker off. On: games get eye tracking too (visual only). Applies to the next Play; Work has its own Eye tracking switch above."), !busy && (app.core.facts.caps.eye || was_on)).changed() {
        app.core.set_eye_games(eye_games);
    }
    let mut auto = app.core.state.autostart;
    if switch_row(ui, &mut auto, "Start at login", Some("When the app opens at login, the side you used last (Play or Work) starts by itself if the headset is connected, the Monado cap is set and the checks are clear. Never asks for a password."), true).changed() {
        app.core.set_autostart(auto);
    }
    if app.core.state.dev_mode {
        let hands_why = app.core.hands_unavailable();
        let r = widgets::setting(ui, "Camera hands", Some("Hand tracking with the front cameras (needs 6DoF). Experimental."), |ui| {
            chip(ui, "beta", theme::pal(ui).warn);
            switch(ui, &mut app.core.toggles.hands, "Camera hands", ed && app.core.toggles.sixdof && (hands_why.is_none() || t.hands))
        });
        if r.changed() {
            app.core.state.hands_chosen = true;
        }
        if let Some(w) = &hands_why {
            r.on_disabled_hover_text(w.as_str());
        }
        crate::pages::tracking::hand_models(ui, app);
        crate::pages::tracking::rules(app);
    }
}

/// Subtitles: live while a desk session runs, else the default of the next start.
pub(crate) fn subtitles_row(ui: &mut Ui, app: &mut PimaxApp) {
    let busy = app.core.busy();
    match app.core.subtitles_live() {
        Some(on) => {
            let mut v = on;
            if switch_row(ui, &mut v, "Subtitles", Some("Live subtitles of what you say, under the main screen (now, in the running desk)."), !busy).changed() {
                app.core.set_subtitles(v);
            }
        }
        None => {
            let mut d = app.core.toggles.voice;
            if switch_row(ui, &mut d, "Subtitles", Some("Live subtitles of what you say, under the main screen (from the next Work start)."), !busy).changed() {
                app.core.set_subtitles_default(d);
            }
        }
    }
}

/// The tracking mode, 6DoF, controllers in Work, the room and the gyro.
fn tracking_card(ui: &mut Ui, app: &mut PimaxApp, mode: &(String, Dev)) {
    let p = theme::pal(ui);
    widgets::title_row(ui, "Tracking", None, |ui| {
        chip(ui, &mode.0, if mode.1 == Dev::Off { p.dim } else { widgets::dev_color(&p, mode.1) });
    });
    ui.add_space(4.0);
    let ed = editable(app);
    let caps_ = app.core.facts.caps.clone();
    let t = app.core.toggles;
    switch_row(ui, &mut app.core.toggles.sixdof, "6DoF", Some("Head tracking with the headset cameras. Start it in a lit room with the cameras free."), ed && (caps_.sixdof || t.sixdof));
    let mut use_ctrl = app.core.wayvr.settings.controllers_in_work;
    let r = switch_row(ui, &mut use_ctrl, "Controllers in Work", Some("Off (default): with camera hands on, your bare hands are the pointer in Work and the controllers are ignored. Voice: \"use controllers\" / \"hands only\"."), ed && app.core.toggles.hands);
    if r.changed() {
        app.core.set_controllers_in_work(use_ctrl);
    }
    crate::pages::tracking::rules(app);
    let room = crate::pages::room::summary(app);
    widgets::setting(ui, "Room", None, |ui| {
        ui.label(RichText::new(if room.is_empty() { "not set up".to_string() } else { room }).size(12.0).color(p.dim));
        if widgets::ghost(ui, icon::CARET_RIGHT, "Set up", true).on_hover_text("The Room page: room setup, the map and the floor").clicked() {
            app.core.page = Page::Room;
        }
    });
    if app.core.live.service.is_some() {
        let gyro = if app.core.live.log.gyro_ready { "calibrated" } else { "calibrating" };
        widgets::setting(ui, "Gyro", None, |ui| {
            ui.label(RichText::new(gyro).size(12.0).color(p.dim));
        });
    }
}

/// Sound and microphone in the headset, with their tests.
fn sound_card(ui: &mut Ui, app: &mut PimaxApp) {
    let p = theme::pal(ui);
    widgets::title_row(ui, "Sound", None, |_| {});
    ui.add_space(4.0);
    let free = !app.core.busy();
    let mut on = app.core.facts.sound_headset;
    let mut speaker_test = false;
    let r = widgets::setting(ui, "Headset sound", Some("While VR runs (Play, Work, SteamVR) the sound goes to the headset speakers, over the video cable, a few seconds after the headset lights up. Stop puts the old output back."), |ui| {
        if widgets::ghost(ui, icon::HEADSET, "Test sound", free).on_hover_text("Two soft notes on the headset speaker (while VR runs)").clicked() {
            speaker_test = true;
        }
        switch(ui, &mut on, "Headset sound", free)
    });
    if r.changed() {
        app.core.sound_headset(if on { "on" } else { "off" });
    }
    if speaker_test {
        app.core.speaker_test();
    }
    if let Some(r) = &app.core.speaker_result {
        widgets::status(ui, r, if r.starts_with("played") { p.ok } else { p.warn });
    }
    let mut mic = app.core.facts.mic_headset;
    let mut mic_test = false;
    let r = widgets::setting(ui, "Headset mic", Some("While VR runs the headset microphone is the default input (voice commands, games, calls); Stop puts the old input back. Voice captions always listen to it."), |ui| {
        if widgets::ghost(ui, icon::MICROPHONE, "Test microphone", free).on_hover_text("Records 3 s, shows the level and plays it back (start Play or Work first: the microphone is silent while the display is off)").clicked() {
            mic_test = true;
        }
        switch(ui, &mut mic, "Headset microphone", free)
    });
    if r.changed() {
        app.core.sound_headset(if mic { "mic-on" } else { "mic-off" });
    }
    if mic_test {
        app.core.mic_test();
    }
    let kind = match app.core.facts.mic_kind() {
        Some("headset") => "headset",
        Some(_) => "other",
        None => "unknown",
    };
    widgets::setting(ui, "Input now", None, |ui| {
        ui.label(RichText::new(kind).size(12.0).color(p.dim)).on_hover_text(app.core.facts.mic_default.clone().unwrap_or_else(|| "pactl did not answer".into()));
    });
    if let Some(r) = &app.core.mic_result {
        widgets::status(ui, r, if r.ends_with(": good") { p.ok } else { p.warn });
    }
}

/// Pair one controller, unpair it, make it vibrate.
pub(crate) fn pair_ui(ui: &mut Ui, app: &mut PimaxApp, side: &'static str) {
    pair_buttons(ui, app, side);
    unpair_ui(ui, app, side);
    buzz_ui(ui, app, side);
}

/// "Vibrate": Pimax Play's test pulse (25 %, 300 ms, `58 09 07 SS 19 2c 01`); through the running VR session when
/// there is one. The result shows on the message line.
fn buzz_ui(ui: &mut Ui, app: &mut PimaxApp, side: &'static str) {
    let busy = app.core.task_busy("buzz");
    if widgets::ghost_short(ui, icon::PULSE, "Vibrate", &format!("Vibrate {side}"), !busy)
        .on_hover_text("A short buzz (25 %, 300 ms), like Pimax Play's test. Works with or without VR running; the controller must be awake.")
        .clicked()
    {
        app.core.buzz(side);
    }
}

/// Unpair one controller (`58 03 05 SS`): the button, then a one-line confirm; the result shows on the message line.
fn unpair_ui(ui: &mut Ui, app: &mut PimaxApp, side: &'static str) {
    use omarchyvr_gui_core::auto::PairState;
    let p = theme::pal(ui);
    let id = Id::new(("unpair-confirm", side));
    let asking = ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let pairing = app.core.auto.pair.as_ref().is_some_and(|x| x.2 == PairState::Waiting);
    // who owns the headset (cached 2 s: it scans /proc)
    let oid = Id::new("vr-owner");
    let owner: Option<&'static str> = match ui.ctx().data(|d| d.get_temp::<(std::time::Instant, Option<&'static str>)>(oid)) {
        Some((t, o)) if t.elapsed().as_secs() < 2 => o,
        _ => {
            let o = if app.core.demo { None } else { omarchyvr_gui_core::auto::vr_session_owner() };
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
        ui.label(RichText::new(format!("Unpair the {side} controller? It must be paired again.")).size(11.5).color(p.warn_text));
        if widgets::ghost_short(ui, icon::LINK_SIMPLE, "Yes, unpair", &format!("Yes, unpair {side}"), can).clicked() {
            ui.ctx().data_mut(|d| d.insert_temp(id, false));
            app.core.unpair(side);
        }
        if widgets::ghost(ui, icon::X_CIRCLE, "Cancel", true).clicked() {
            ui.ctx().data_mut(|d| d.insert_temp(id, false));
        }
    } else if widgets::ghost_short(ui, icon::LINK_SIMPLE, "Unpair", &format!("Unpair {side}"), can)
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
    use omarchyvr_gui_core::auto::{PairState, PAIR_HINT, PAIR_SECS};
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
            let r = widgets::ghost_short(ui, icon::LINK_SIMPLE, "Pair again", &format!("Pair {side} again"), can);
            if r.on_disabled_hover_text(wait_text.as_str()).clicked() {
                app.core.pair(side);
            }
        }
        None => {
            if widgets::ghost_short(ui, icon::LINK_SIMPLE, "Pair", &format!("Pair {side}"), can)
                .on_hover_text("Put the controller in pairing mode first, then press this and wait up to 60 s (one hand at a time)")
                .on_disabled_hover_text(wait_text.as_str())
                .clicked()
            {
                app.core.pair(side);
            }
        }
    }
}
