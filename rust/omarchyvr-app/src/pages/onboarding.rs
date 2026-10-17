//! "Setting up your PC": the automatic first-run setup (`omarchyvr_gui_core::autosetup`). It starts by itself when the
//! app opens: the checks, the fixes that need no password, the root job with ONE password prompt (its steps as a live
//! list), the checks again. Then Ready, a "Restart now" button, or a list of problems each with one Fix button.
//! After Ready the steps that need the person in the headset (IPD, controllers, eye calibration, room) are a short
//! optional list, one button each. The old nine-step stepper is gone; "Run setup again" is on the Ready screen.
use crate::app::PimaxApp;
use crate::theme::{self, icon};
use crate::widgets::{self, card, chip, note, pal, spinner, title, Kind};
use eframe::egui::{self, RichText, Ui};
use omarchyvr_gui_core::app::Page;
use omarchyvr_gui_core::autosetup::{AutoSetup, CheckFix, Phase, RunKind, StepState, Stop};
use omarchyvr_gui_core::device::onboarding::Step;
use omarchyvr_gui_core::guistate::SetupStage;
use omarchyvr_gui_core::headset_choice::{self, HeadsetPick};

/// The mark and colour of a step state.
fn mark(ui: &mut Ui, st: StepState) {
    let p = pal(ui);
    let faint = theme::pal(ui).faint;
    match st {
        StepState::Running => {
            spinner(ui);
        }
        StepState::Done => {
            ui.label(RichText::new(icon::CHECK_CIRCLE).color(p.ok));
        }
        StepState::Skipped => {
            ui.label(RichText::new("-").color(faint));
        }
        StepState::Failed => {
            ui.label(RichText::new(icon::X_CIRCLE).color(p.fail));
        }
        StepState::Pending => {
            ui.label(RichText::new(icon::CIRCLE).color(faint));
        }
    }
}

/// Draw the page.
pub fn show(ui: &mut Ui, app: &mut PimaxApp) {
    if let Some(w) = app.core.onboarding.wake_only.clone() {
        wake_only(ui, app, &w);
        return;
    }
    app.core.want_setup();
    let s = app.core.onboarding.setup.clone();
    let done = app.core.state.setup == SetupStage::Done;
    match &s.phase {
        Phase::Idle if done => ready(ui, app, &s),
        Phase::Idle => {
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    spinner(ui);
                    ui.label(RichText::new("Getting ready to set up your PC...").color(pal(ui).muted));
                });
            });
        }
        Phase::ChooseHeadset => choose_headset(ui, app, &s),
        Phase::GetReady => get_ready(ui, app, &s),
        Phase::Looking | Phase::FixingUser | Phase::Password | Phase::Verifying => running(ui, app, &s),
        Phase::NeedsRestart => restart(ui, app, &s),
        Phase::Problems => problems(ui, app, &s),
        Phase::PasswordStopped(stop) => stopped(ui, app, &s, stop),
        Phase::Cancelled => {
            header(ui, "Setup was cancelled", &s.headline());
            card(ui, |ui| {
                note(ui, "Nothing more was changed. Press Try again whenever you are ready.");
                ui.add_space(6.0);
                if widgets::primary(ui, "Try again", true).clicked() {
                    app.core.setup_retry();
                }
            });
        }
        Phase::Ready => ready(ui, app, &s),
    }
}

/// First screen: "Which headset do you have?". USB detection only pre-selects; the person confirms with Continue.
fn choose_headset(ui: &mut Ui, app: &mut PimaxApp, s: &AutoSetup) {
    header(ui, "Which headset do you have?", "Pick yours. The setup does only what that headset needs. Nothing is changed before you press Continue.");
    card(ui, |ui| {
        if let Some(found) = &s.found {
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon::CHECK_CIRCLE).color(pal(ui).ok));
                ui.label(RichText::new(found).color(theme::pal(ui).text));
            });
            ui.add_space(6.0);
        }
        let mut entries: Vec<(HeadsetPick, String, String)> = headset_choice::models()
            .iter()
            .enumerate()
            .map(|(i, m)| (HeadsetPick::Model(i as u8), m.descriptor().name.clone(), "Supported and tested".to_string()))
            .collect();
        entries.push((HeadsetPick::Other, "Other / not listed".into(), "Not supported yet".into()));
        for (pick, name, sub) in entries {
            let on = s.selected == pick;
            let p = pal(ui);
            let r = ui
                .horizontal(|ui| {
                    ui.label(RichText::new(if on { icon::CHECK_CIRCLE } else { icon::CIRCLE }).color(if on { p.ok } else { theme::pal(ui).faint }));
                    ui.selectable_label(on, RichText::new(&name).strong()).on_hover_text(&sub)
                })
                .inner;
            ui.indent(("headset-sub", &name), |ui| ui.label(RichText::new(&sub).small().color(theme::pal(ui).faint)));
            if r.clicked() {
                app.core.setup_select_headset(pick);
            }
            ui.add_space(4.0);
        }
        if s.selected == HeadsetPick::Other {
            ui.add_space(4.0);
            note(ui, "This headset is not supported yet. The setup still installs the app, but makes no headset-specific changes to this PC (no graphics driver patch, no display override).");
        }
        ui.add_space(8.0);
        if widgets::primary(ui, "Continue", s.selected != HeadsetPick::Unset).on_disabled_hover_text("Choose your headset first").clicked() {
            app.core.setup_headset_continue();
        }
        if s.selected == HeadsetPick::Unset {
            ui.add_space(4.0);
            note(ui, if s.found.is_none() { "No headset was found on USB, so nothing is selected. Choose yours above." } else { "Choose your headset above." });
        }
    });
}

/// Second screen: plain steps for the chosen headset, live ticks from detection, "Start setup" (always enabled).
fn get_ready(ui: &mut Ui, app: &mut PimaxApp, s: &AutoSetup) {
    header(ui, "Get the headset ready", &format!("{}: do these, then start the setup.", s.selected.title()));
    let steps = app.core.headset_ready_steps();
    card(ui, |ui| {
        for st in &steps {
            ui.horizontal_wrapped(|ui| {
                match st.seen {
                    Some(true) => ui.label(RichText::new(icon::CHECK_CIRCLE).color(pal(ui).ok)),
                    Some(false) => ui.label(RichText::new(icon::CIRCLE).color(theme::pal(ui).faint)),
                    None => ui.label(RichText::new("-").color(theme::pal(ui).faint)),
                };
                ui.label(RichText::new(&st.text).color(theme::pal(ui).text));
                match st.seen {
                    Some(true) => ui.label(RichText::new("seen").small().color(pal(ui).ok)),
                    Some(false) => ui.label(RichText::new("not seen yet").small().color(theme::pal(ui).faint)),
                    None => ui.label(RichText::new("").small()),
                };
            });
        }
        if s.selected.module().is_some_and(|m| m.id() == "dream-air") {
            ui.add_space(8.0);
            wake_guide(ui, app);
        }
        let missing = steps.iter().any(|x| x.seen == Some(false));
        ui.add_space(8.0);
        if missing {
            note(ui, "Something is not seen yet. You can start anyway: a sleeping headset wakes during the setup. If the display is not found, the app tells you and you can press Fix later.");
            ui.add_space(4.0);
        }
        ui.horizontal(|ui| {
            if widgets::primary(ui, "Start setup", true).clicked() {
                app.core.setup_get_ready_start();
            }
            if widgets::tbutton(ui, "Back", true).clicked() {
                app.core.onboarding.setup.headset_back();
            }
        });
    });
}

/// "Get the headset ready" on its own: a Work/Play start or a headset restart found the headset asleep or its cameras
/// gone (2026-10-17 13:10: Work only said "camera recovery failed"). The reason, the guide, and Back.
fn wake_only(ui: &mut Ui, app: &mut PimaxApp, w: &omarchyvr_gui_core::onboarding::WakeOnly) {
    header(ui, "Get the headset ready", &w.reason);
    card(ui, |ui| {
        wake_guide(ui, app);
        ui.add_space(8.0);
        let awake = app.core.onboarding.wake.is_awake();
        if awake {
            note(ui, "The headset is ready. Go back and start again.");
            ui.add_space(4.0);
        }
        if widgets::primary(ui, "Back", true).clicked() {
            app.core.close_wake_guide();
        }
    });
}

/// The live state of the headset's HID chip (and, when the guide waits for them, its cameras) and, when it is not
/// ready, the two guided steps with their animations.
fn wake_guide(ui: &mut Ui, app: &mut PimaxApp) {
    use omarchyvr_gui_core::wake::Wake;
    let (view, t) = app.core.wake_view(ui.input(|i| i.time));
    let p = pal(ui);
    let faint = theme::pal(ui).faint;
    let text = theme::pal(ui).text;
    let line = |ui: &mut Ui, mark: &str, color: egui::Color32, head: &str| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(mark).color(color));
            ui.label(RichText::new(head).strong().color(text));
        });
    };
    // the cameras too (a start or the setup found them gone), or only the control chip
    let cameras = app.core.wake_target() == Some(true);
    match view {
        Wake::Idle => {}
        Wake::NotConnected => {
            line(ui, icon::CIRCLE, faint, "Headset not connected");
            note(ui, "Nothing of the headset is on USB. Plug in its USB cable and its power cable.");
        }
        Wake::Awake => line(ui, icon::CHECK_CIRCLE, p.ok, if cameras { "Headset and its cameras are on" } else { "Headset is awake" }),
        Wake::PressPower { left } => {
            line(ui, icon::CIRCLE, p.warn, if cameras { "The headset's cameras are off: step A, press the power button" } else { "Headset is asleep or off: step A, press the power button" });
            note(ui, "Press the power button once on the headset. This page notices by itself when it wakes up.");
            crate::wake_art::show(ui, crate::wake_art::Anim::Press, t);
            ui.label(RichText::new(format!("Waiting {} s, then the next step is shown", left.ceil() as u32)).small().color(faint));
        }
        Wake::UnplugCable | Wake::WaitUnplugged { .. } | Wake::PlugBack | Wake::WaitWake { .. } | Wake::BlockedBySession => {
            line(ui, icon::CIRCLE, p.warn, if cameras { "Cameras still off: step B, the last resort: unplug the headset's power cable" } else { "Still asleep: step B, the last resort: unplug the headset's power cable" });
            note(ui, "Unplug the headset's POWER cable, wait 10 seconds, plug it back in. Leave the DisplayPort cable in.");
            crate::wake_art::show(ui, crate::wake_art::Anim::Cable, t);
            match view {
                Wake::UnplugCable => {
                    ui.label(RichText::new("Unplug the power cable now. This page sees it when the headset leaves USB.").small().color(faint));
                    if widgets::tbutton(ui, "I unplugged it", true).clicked() {
                        app.core.wake_unplugged_pressed();
                    }
                }
                Wake::WaitUnplugged { left } => {
                    ui.label(RichText::new(format!("Wait {} more seconds with the cable out.", left.ceil() as u32)).color(text));
                }
                Wake::PlugBack => {
                    ui.label(RichText::new("Plug the power cable back in now.").strong().color(p.accent));
                }
                Wake::WaitWake { left } => {
                    ui.label(RichText::new(format!("Cable is back. Waiting for the headset to wake up ({} s)...", left.ceil() as u32)).small().color(faint));
                }
                _ => {
                    ui.label(RichText::new("A VR session is running, so nothing is asked of you. Stop it first.").small().color(p.warn));
                }
            }
        }
        Wake::GaveUp => {
            line(ui, icon::X_CIRCLE, p.warn, if cameras { "The cameras did not come back" } else { "The headset did not wake up" });
            if app.core.onboarding.setup.phase == Phase::GetReady && app.core.onboarding.wake_only.is_none() {
                note(ui, "You can still start the setup: it installs the permission rule and the keep-alive, which can wake it. If the display is not found afterwards, the result screen tells you and offers the Fix.");
            } else {
                note(ui, "Leave the headset a minute with both cables in, then try the steps again. If it still does not come back, send a report (Help > Report a problem).");
            }
            if widgets::tbutton(ui, "Try the steps again", true).clicked() {
                app.core.wake_retry();
            }
        }
    }
}

/// The NVIDIA patch card only when it matters: the driver is not a tested version (the switch "Allow the patch on
/// untested NVIDIA drivers" then needs a Run setup again), or Developer mode is on.
fn untested_card(ui: &mut Ui, app: &mut PimaxApp) {
    if app.core.onboarding.install.nvidia.tested == Some(false) || app.core.state.dev_mode {
        crate::pages::setup::nvidia_card(ui, app);
    }
}

/// The title card.
fn header(ui: &mut Ui, text: &str, sub: &str) {
    card(ui, |ui| {
        title(ui, text);
        note(ui, sub);
    });
}

/// The four stages and, under the password stage, the live list of the root job.
fn stages(ui: &mut Ui, s: &AutoSetup) {
    let after = s.kind.check_only();
    let names = [
        "Look at this PC",
        "Fix what needs no password",
        "System setup (one password)",
        match s.kind {
            RunKind::AfterReboot => "Check after the restart",
            RunKind::AfterUpdate => "Check after the update",
            _ => "Check everything again",
        },
    ];
    let skipped_note = ["", "nothing to fix", "nothing needs a password", ""];
    for (i, name) in names.iter().enumerate() {
        ui.horizontal(|ui| {
            mark(ui, s.stages[i]);
            let c = match s.stages[i] {
                StepState::Pending => theme::pal(ui).faint,
                StepState::Failed => pal(ui).fail,
                StepState::Running => theme::pal(ui).text,
                _ => pal(ui).muted,
            };
            ui.label(RichText::new(*name).color(c).strong());
            if s.stages[i] == StepState::Skipped && !skipped_note[i].is_empty() && !after {
                ui.label(RichText::new(skipped_note[i]).small().color(theme::pal(ui).faint));
            }
        });
        if i == 2 && !s.root_steps.is_empty() && s.stages[2] != StepState::Skipped && !after {
            ui.indent("setup-root-steps", |ui| {
                for st in &s.root_steps {
                    ui.horizontal(|ui| {
                        mark(ui, st.state);
                        let c = match st.state {
                            StepState::Pending => theme::pal(ui).faint,
                            StepState::Failed => pal(ui).fail,
                            StepState::Running => theme::pal(ui).text,
                            _ => pal(ui).muted,
                        };
                        ui.label(RichText::new(&st.title).size(13.0).color(c));
                        if !st.note.is_empty() && matches!(st.state, StepState::Skipped | StepState::Failed) {
                            ui.label(RichText::new(&st.note).small().color(theme::pal(ui).faint));
                        }
                    });
                }
            });
        }
    }
}

/// "Show details": the exact plan or log text.
fn details(ui: &mut Ui, s: &AutoSetup, extra: &[String]) {
    egui::CollapsingHeader::new(RichText::new("Show details").small()).id_salt("setup-details").show(ui, |ui| {
        let mut lines: Vec<String> = s.raw.lines().map(str::to_string).collect();
        if lines.is_empty() {
            lines.push("(nothing ran yet)".into());
        }
        widgets::code_box(ui, &lines);
        if !extra.is_empty() {
            ui.add_space(4.0);
            widgets::code_box(ui, extra);
        }
    });
}

fn running(ui: &mut Ui, app: &mut PimaxApp, s: &AutoSetup) {
    let title = match s.kind {
        RunKind::AfterReboot => "Finishing the setup",
        RunKind::AfterUpdate => "Checking after the update",
        _ => "Setting up your PC",
    };
    header(ui, title, &s.headline());
    card(ui, |ui| {
        stages(ui, s);
        ui.add_space(8.0);
        if s.phase == Phase::Password {
            note(ui, "The password window comes from your desktop. One password does all of it; nothing else changes on this PC.");
        }
        ui.add_space(4.0);
        details(ui, s, &[]);
        ui.add_space(6.0);
        let stop = !s.cancel && !matches!(s.phase, Phase::Password);
        if s.cancel {
            note(ui, "Stopping after the step that runs now...");
        } else if widgets::tbutton(ui, "Cancel", stop).on_disabled_hover_text("The system setup cannot be interrupted once it has started").clicked() {
            app.core.setup_cancel();
        }
    });
}

fn restart(ui: &mut Ui, app: &mut PimaxApp, s: &AutoSetup) {
    header(ui, "Everything is set up", "One restart is left. The patched graphics driver and the headset's display mode start working after it.");
    card(ui, |ui| {
        stages(ui, s);
        ui.add_space(10.0);
        let w = ui.available_width().min(420.0);
        if widgets::action(ui, icon::POWER, "Restart now", Kind::Primary, !app.core.busy(), w).on_hover_text("Restarts this PC. The app opens again by itself and finishes the checks.").clicked() {
            app.core.setup_restart_now();
        }
        ui.add_space(4.0);
        if s.later {
            note(ui, "Restart whenever you like. The app opens again after your next restart and finishes the checks by itself. VR needs that restart.");
        } else if widgets::tbutton(ui, "Later", true).clicked() {
            app.core.setup_restart_later();
        }
        ui.add_space(4.0);
        details(ui, s, &[]);
    });
}

fn problems(ui: &mut Ui, app: &mut PimaxApp, s: &AutoSetup) {
    header(ui, "A few things need your attention", &s.headline());
    let busy = app.core.busy();
    let root = s.problems.iter().filter(|p| matches!(p.fix, CheckFix::Root { .. })).count();
    // ONE password: with two root fixes or more only "Fix all" asks (2026-10-17 12:40: two Fix buttons, two prompts
    // 4 s apart)
    let together = root >= 2;
    let guided = omarchyvr_gui_core::autosetup::guide_for(&s.problems).is_some();
    let manual = s.problems.iter().any(|p| p.fix == CheckFix::None);
    let good = s.checks.iter().filter(|c| c.state != omarchyvr_gui_core::autosetup::CheckState::Fail).count();
    card(ui, |ui| {
        for p in &s.problems {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(icon::X_CIRCLE).color(pal(ui).fail));
                ui.label(RichText::new(&p.title).color(theme::pal(ui).text).strong());
            });
            ui.indent(("problem", &p.id), |ui| {
                note(ui, &p.reason);
                match &p.fix {
                    // the guide below covers it and the list checks again by itself when the headset is back
                    CheckFix::None if guided && matches!(p.id.as_str(), "headset" | "connector") => {
                        ui.label(RichText::new("Follow the steps below; this list checks again by itself.").small().color(pal(ui).muted));
                    }
                    CheckFix::None => {
                        ui.label(RichText::new("You do this one yourself, then press Check again.").small().color(pal(ui).muted));
                    }
                    CheckFix::User => {
                        if widgets::primary(ui, "Fix", !busy).clicked() {
                            app.core.setup_fix_one(&p.id);
                        }
                    }
                    CheckFix::Root { .. } if together => {
                        ui.label(RichText::new("Fixed together with the others below (one password).").small().color(pal(ui).muted));
                    }
                    CheckFix::Root { .. } => {
                        ui.horizontal(|ui| {
                            if widgets::primary(ui, "Fix", !busy).on_hover_text("Asks for your password").clicked() {
                                app.core.setup_fix_one(&p.id);
                            }
                            chip(ui, "password", pal(ui).warn);
                        });
                    }
                }
            });
            ui.add_space(6.0);
        }
        if good > 0 {
            ui.label(RichText::new(format!("{good} other check{} fine.", if good == 1 { " is" } else { "s are" })).small().color(pal(ui).ok));
            ui.add_space(4.0);
        }
        ui.horizontal(|ui| {
            if together && widgets::primary(ui, "Fix all (one password)", !busy).clicked() {
                app.core.setup_fix_all();
            }
            if manual && widgets::tbutton(ui, "Check again", !busy).clicked() {
                app.core.setup_check_again();
            }
        });
        ui.add_space(4.0);
        let extra: Vec<String> = s.problems.iter().map(|p| format!("{}: {}", p.id, p.detail)).collect();
        details(ui, s, &extra);
    });
    // the headset is asleep or its cameras are off: the guide, live (it checks again by itself when they are back)
    if guided {
        card(ui, |ui| {
            title(ui, "Get the headset ready");
            wake_guide(ui, app);
        });
    }
    // the system part already asked for a restart: it stays one press away, so nobody edits the setup state by hand
    // (2026-10-17 12:46). Finishing the items first saves a second restart when one of them needs one too.
    if s.reboot && !s.kind.check_only() {
        card(ui, |ui| {
            note(ui, "The system part is done and needs one restart. Restart now and finish the items above afterwards (the app opens again by itself), or finish them first: then one restart covers everything.");
            ui.add_space(6.0);
            let w = ui.available_width().min(420.0);
            if widgets::action(ui, icon::POWER, "Restart now", Kind::Secondary, !busy, w).on_hover_text("Restarts this PC. The app opens again by itself and finishes the checks.").clicked() {
                app.core.setup_restart_now();
            }
        });
    }
    untested_card(ui, app);
}

fn stopped(ui: &mut Ui, app: &mut PimaxApp, s: &AutoSetup, stop: &Stop) {
    header(ui, "The system setup did not run", &s.headline());
    card(ui, |ui| {
        stages(ui, s);
        ui.add_space(6.0);
        note(
            ui,
            match stop {
                Stop::Cancelled => "The password window was closed. Press Try again and type your password when it asks.",
                Stop::WrongPassword => "Check the password and press Try again. If it keeps failing, this account may not be allowed to change the system.",
                Stop::NoDialog => "No password window (polkit agent) is running on this desktop. Log out and in again, then press Try again.",
            },
        );
        ui.add_space(6.0);
        if widgets::primary(ui, "Try again", !app.core.busy()).clicked() {
            app.core.setup_retry();
        }
        ui.add_space(4.0);
        details(ui, s, &[]);
    });
}

/// The Ready screen: Play and Work, the optional steps in the headset, "Run setup again".
fn ready(ui: &mut Ui, app: &mut PimaxApp, s: &AutoSetup) {
    let after = s.kind == RunKind::AfterReboot && s.phase == Phase::Ready;
    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::CHECK_CIRCLE).size(26.0).color(pal(ui).ok));
            title(ui, "Your PC is ready");
        });
        note(ui, if after { "Checked after the restart: the patched driver is loaded, the headset display mode is in place and VR has realtime priority." } else { "Everything is set up and checked." });
        ui.add_space(8.0);
        let half = ((ui.available_width() - 10.0) / 2.0).max(120.0);
        let busy = app.core.busy();
        let first = if app.core.work_setup_done() { "" } else { " The first time, two questions come first (voice assistant, speech)." };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            if super::home::big_choice(ui, icon::GAME_CONTROLLER, "Play", "Games: SteamVR and OpenXR", !busy, half)
                .on_hover_text("Starts VR for games with SteamVR and the headset: the whole graphics card is the game's")
                .clicked()
            {
                app.core.page = Page::Desk;
                app.core.play();
            }
            if super::home::big_choice(ui, icon::DESKTOP, "Work", "Desk: screens, voice, AI", !busy, half)
                .on_hover_text(format!("The WayVR desk: screens, passthrough, voice, the AI.{first}"))
                .clicked()
            {
                app.core.page = Page::Desk;
                app.core.work();
            }
        });
        if !s.checks.is_empty() {
            ui.add_space(4.0);
            details(ui, s, &s.checks.iter().map(|c| format!("ok  {}", c.title)).collect::<Vec<_>>());
        }
    });
    next_in_headset(ui, app);
    untested_card(ui, app);
    card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            if widgets::ibutton(ui, icon::ARROW_COUNTER_CLOCKWISE, "Run setup again", !app.core.busy()).on_hover_text("Runs the checks and the system setup once more (one password if it needs one)").clicked() {
                app.core.setup_rerun();
            }
            if app.core.state.dev_mode && widgets::ibutton(ui, icon::WRENCH, "Developer: checks and fixes", true).on_hover_text("The full Setup page: every check, builds, autofix").clicked() {
                app.core.page = Page::Setup;
            }
        });
    });
}

/// "Next, in the headset": optional, each one button; the guided in-VR flows are the existing ones.
fn next_in_headset(ui: &mut Ui, app: &mut PimaxApp) {
    app.core.want_room();
    let busy = app.core.busy();
    let ipd = app.core.facts.ipd_mm.map(|mm| format!("saved: {mm:.1} mm"));
    let eye = app.core.facts.eye_calibration.then(|| "saved".to_string());
    let room = (!app.core.room.state.maps.is_empty()).then(|| format!("{} map{}", app.core.room.state.maps.len(), if app.core.room.state.maps.len() == 1 { "" } else { "s" }));
    let c = &app.core.live.log.ctrl;
    let left = app.core.onboarding.paired[0] || c.slots_seen.contains(&omarchyvr_gui_core::devices::SLOT_LEFT);
    let right = app.core.onboarding.paired[1] || c.slots_seen.contains(&omarchyvr_gui_core::devices::SLOT_RIGHT);
    let pair = (left && right).then(|| "both paired".to_string());
    let rows: [(Step, &str, &str, &str, Option<String>); 4] = [
        (Step::Ipd, icon::EYE, "Set the lens distance (IPD)", "Measure and set IPD", ipd),
        (Step::Pair, icon::GAME_CONTROLLER, "Pair the controllers", "Pair controllers", pair),
        (Step::EyeCal, icon::CROSSHAIR, "Calibrate eye tracking", "Calibrate eye tracking", eye),
        (Step::Room, icon::MAP_TRIFOLD, "Map your room", "Open Room setup", room),
    ];
    let mut run = None;
    widgets::section(ui, "Next, in the headset", Some("Optional. These need you wearing the headset, so they are not part of the setup. The guided steps talk to you inside the headset."), |ui| {
        for (step, ic, name, button, state) in &rows {
            ui.horizontal(|ui| {
                ui.label(RichText::new(*ic).color(pal(ui).muted));
                ui.label(RichText::new(*name).color(theme::pal(ui).text).strong());
                match state {
                    Some(t) => chip(ui, t, pal(ui).ok),
                    None => chip(ui, "not done", pal(ui).muted),
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::tbutton(ui, button, !busy).clicked() {
                        run = Some(*step);
                    }
                });
            });
        }
    });
    match run {
        Some(Step::Pair) => app.core.page = Page::Desk,
        Some(s) => app.core.onboarding_run(s),
        None => {}
    }
}
