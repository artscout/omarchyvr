//! Pieces of the desk session shown on the Home page: the preflight banner (LESSONS rows 1-10), the live status of
//! a running session and what Start runs.
use crate::app::PimaxApp;
use crate::widgets::{self, card, chip, kv, note, pal, sev_chip, sev_color, title};
use eframe::egui::{self, RichText, Ui};
use omarchyvr_gui_core::launcher;
use omarchyvr_gui_core::monlog::TrackState;
use omarchyvr_gui_core::preflight::{Action, Gate, Preflight, Sev};
use omarchyvr_gui_core::procs::fmt_age;

fn action_label(a: &Action, running: bool) -> String {
    match a {
        Action::StopSession => "Stop session".into(),
        Action::StopForeignMonado => "Stop it (vr stop --grace 30 --all)".into(),
        Action::InterruptPid { pid, comm } => format!("SIGINT {comm} ({pid})"),
        Action::Setup(_) => "Open in Setup".into(),
        Action::WithoutEye => {
            if running {
                "Restart without eye tracking".into()
            } else {
                "Start without eye tracking".into()
            }
        }
    }
}

/// The preflight banner in a card (the wizard's last step).
pub fn banner(ui: &mut Ui, app: &mut PimaxApp, pf: &Preflight) {
    card(ui, |ui| {
        ui.horizontal(|ui| {
            title(ui, "Preflight");
            let (t, c) = match &pf.gate {
                Gate::Ready => ("ready to start", pal(ui).ok),
                Gate::NeedsConsent(_) => ("needs your OK", pal(ui).warn),
                Gate::Blocked(_) => ("blocked", pal(ui).fail),
            };
            chip(ui, t, c);
        });
        ui.add_space(4.0);
        banner_body(ui, app, pf);
        if matches!(pf.gate, Gate::NeedsConsent(_)) {
            ui.add_space(4.0);
            ui.checkbox(
                &mut app.core.consent_force,
                RichText::new("Start anyway: the only problem is WayVR Xid 31 faults (adds --force)").color(pal(ui).warn),
            )
            .on_hover_text("LESSONS row 9: per-app MMU faults the GPU recovers from. Any other Xid or a display engine error blocks the start.");
        }
    });
}

/// The findings of a preflight with their buttons (problems first, the passed checks folded).
pub fn banner_body(ui: &mut Ui, app: &mut PimaxApp, pf: &Preflight) {
    let mut clicked: Option<Action> = None;
    let running = app.core.live.service.is_some();
    let (passed, rest): (Vec<_>, Vec<_>) = pf.findings.iter().partition(|f| f.sev == Sev::Ok);
    for f in rest {
        ui.horizontal_wrapped(|ui| {
            sev_chip(ui, f.sev);
            ui.label(RichText::new(&f.title).strong().color(if f.sev >= Sev::Warn { sev_color(ui, f.sev) } else { ui.visuals().text_color() }));
            widgets::row_tag(ui, f.row);
            for a in &f.actions {
                if ui.small_button(action_label(a, running)).clicked() {
                    clicked = Some(a.clone());
                }
            }
        });
        if !f.detail.is_empty() {
            ui.indent(f.id, |ui| note(ui, &f.detail));
        }
        ui.add_space(3.0);
    }
    egui::CollapsingHeader::new(format!("{} checks passed", passed.len())).id_salt("passed").show(ui, |ui| {
        for f in passed {
            ui.horizontal_wrapped(|ui| {
                sev_chip(ui, f.sev);
                ui.label(&f.title);
                widgets::row_tag(ui, f.row);
            });
            if !f.detail.is_empty() {
                ui.indent(f.id, |ui| note(ui, &f.detail));
            }
        }
    });
    if let Some(a) = clicked {
        app.core.act(&a);
        app.ui.pf = None;
    }
}

/// The live status of a session (processes, 6DoF, eye gaze, controllers, camera, clients, display).
pub fn status(ui: &mut Ui, app: &mut PimaxApp) {
    let live = app.core.live.clone();
    let p = pal(ui);
    {
        if live.service.is_none() && !live.session_helpers_alive() {
            note(ui, "Nothing is running. The status of a session shows here (processes, 6DoF tracking, eye gaze, controllers).");
            if let Some(f) = live.foreign_monado.first() {
                ui.colored_label(p.fail, format!("monado-service pid {f} runs outside the launcher session."));
            }
            return;
        }
        egui::Grid::new("live").num_columns(2).spacing([18.0, 6.0]).striped(true).show(ui, |ui| {
            if let Some(s) = &live.service {
                kv(ui, "Monado", format!(
                    "pid {}, up {}, {}{}",
                    s.pid,
                    s.age.map(fmt_age).unwrap_or_default(),
                    s.build.as_ref().map(|b| b.display().to_string()).unwrap_or_default(),
                    s.ipd.as_ref().map(|i| format!(", IPD {i} mm")).unwrap_or_default()
                ));
            }
            let procs: Vec<String> = live
                .session
                .iter()
                .filter(|x| x.name != "monado")
                .map(|x| match (x.alive, x.pid) {
                    (true, Some(pid)) => format!("{} {} ({})", x.name, pid, x.age.map(fmt_age).unwrap_or_default()),
                    _ => format!("{} -", x.name),
                })
                .collect();
            kv(ui, "Processes", procs.join("   "));
            let l = &live.log;
            let six = &l.sixdof;
            ui.label(RichText::new("6DoF").color(p.muted));
            ui.horizontal_wrapped(|ui| {
                let (t, c) = match six.state {
                    TrackState::Tracking => ("6DoF tracking", p.ok),
                    TrackState::Lost => ("lost (3DoF, position held)", p.fail),
                    TrackState::Waiting => ("waiting for the tracker (3DoF)", p.warn),
                    TrackState::Off => ("off (3DoF)", p.muted),
                };
                chip(ui, t, c);
                if six.state != TrackState::Off {
                    ui.label(format!(
                        "{} acquired, {} lost, {} resets; {} frame sets, {} IMU samples{}",
                        six.acquisitions,
                        six.losses,
                        six.resets,
                        six.frame_sets,
                        six.imu_samples,
                        six.correction.map(|(mm, deg)| format!("; correction {mm:.1} mm / {deg:.2} deg")).unwrap_or_default()
                    ));
                    if !six.reason.is_empty() && six.state != TrackState::Tracking {
                        ui.label(RichText::new(format!("({})", six.reason)).color(p.warn_text));
                    }
                }
            });
            ui.end_row();
            if six.no_camera_code {
                kv(ui, "", RichText::new("This Monado was built without -DPIMAX_CAM=ON: 6DoF requested but it runs 3DoF (LESSONS row 6). Setup > Rebuild Monado.").color(p.fail));
            }
            let eye = &l.eye;
            let eye_text = match (eye.enabled, eye.session, eye.stats) {
                (false, _, _) => "off".to_string(),
                (true, None, _) => "enabled; no app has asked for gaze yet".to_string(),
                (true, Some(false), _) => "start failed".to_string(),
                (true, Some(true), Some(s)) => format!(
                    "{:.0} % valid of {} gaze queries in {:.0} s, chip {:.0} samples/s",
                    s.valid_pct(),
                    s.queries,
                    s.secs,
                    s.chip_hz
                ),
                (true, Some(true), None) => "session started".to_string(),
            };
            kv(ui, "Eye gaze", eye_text);
            if eye.chip_busy {
                kv(ui, "", RichText::new("The eye chip was held by another process when Monado opened it (LESSONS row 3): quit pimax-eye watch / ipd.").color(p.warn_text));
            }
            let c = &l.ctrl;
            let hands = |h: Option<bool>| match h {
                Some(true) => "active",
                Some(false) => "inactive",
                None => "-",
            };
            let mut ctrl = format!(
                "{} seen (slots {:?}); hands left {}, right {}",
                c.slots_seen.len(),
                c.slots_seen.iter().collect::<Vec<_>>(),
                hands(c.hands[0]),
                hands(c.hands[1])
            );
            if c.optical {
                ctrl += &format!(
                    "; optical {}: {} fixes, {} lost",
                    if c.tracking { "tracking" } else { "searching" },
                    c.fixes,
                    c.fix_lost
                );
            }
            kv(ui, "Controllers", ctrl);
            if let Some(s) = &live.ctrltrack {
                kv(ui, "Controller tracker", s.clone());
            }
            let cam = &l.camera;
            if cam.up || cam.fps.is_some() {
                kv(ui, "Camera", format!(
                    "{}{}{}{}",
                    cam.fps.map(|f| format!("{f:.1} fps")).unwrap_or_else(|| "-".into()),
                    cam.lost.map(|n| format!(", {n} lost")).unwrap_or_default(),
                    if cam.broker { ", via broker" } else { "" },
                    if cam.ae { ", auto exposure on" } else { "" }
                ));
            }
            if cam.ae_stage > 0 {
                kv(ui, "Camera light", format!("dim room: exposure range extended ({})", if cam.ae_stage == 1 { "more gain" } else { "more gain and longer exposure" }));
            }
            if cam.light_low {
                kv(ui, "Camera light", RichText::new(omarchyvr_gui_core::monlog::LIGHT_LOW_TEXT).color(p.warn_text));
            }
            for e in &cam.errors {
                kv(ui, "", RichText::new(e).color(p.fail));
            }
            kv(ui, "Clients", format!("{} connected: {}", l.clients.max(0), l.apps.join(", ")));
            let rate = match (l.video_hz, live.service.as_ref().and_then(|s| s.hz)) {
                (Some(v), Some(h)) if v != h => RichText::new(format!("headset told {v} Hz, service asked {h} Hz")).color(p.warn_text),
                (Some(v), _) => RichText::new(format!("{v} Hz announced to the headset{}", if l.gyro_ready { ", gyro calibration done" } else { "" })),
                _ => RichText::new("-"),
            };
            kv(ui, "Display", rate);
            for e in l.errors.iter().rev().take(3) {
                kv(ui, "Log error", RichText::new(e).color(p.fail).monospace());
            }
        });
    }
}

/// What Start runs: the exact command and the launcher's plan.
pub fn plan_body(ui: &mut Ui, app: &mut PimaxApp) {
    let t = app.core.toggles.supported(&app.core.facts.caps);
    let mut line = String::new();
    for (k, v) in app.core.launcher_env() {
        line += &format!("{k}={v} ");
    }
    line += &format!("{} {}", app.core.launcher_bin().display(), launcher::desk_start_args(&t, app.core.consent_force).join(" "));
    widgets::code_box(ui, &[line]);
    match app.core.plan().unwrap_or_else(|| Err("(reading the launcher's plan...)".into())) {
        Ok(p) => {
            egui::Grid::new("plan-grid").num_columns(2).show(ui, |ui| {
                kv(ui, "Monado build", p.monado_build.display().to_string());
                if !p.sixdof_bins.is_empty() {
                    kv(ui, "First", "pimax-cam broker (cameras shared; stopped last)");
                }
                kv(ui, "Then", p.clients.iter().map(|c| c.name.clone()).collect::<Vec<_>>().join(", "));
                kv(ui, "Session dir", p.session_dir.display().to_string());
            });
        }
        Err(e) => {
            ui.colored_label(pal(ui).fail, e);
        }
    }
    note(ui, "Before it, the app waits until no monado-service is left (up to 45 s) and checks the camera and the eye chip again. The environment builder of the launcher sets the compositor (DP-1, mode 2/3, Wayland direct); the app never starts Monado itself.");
}
