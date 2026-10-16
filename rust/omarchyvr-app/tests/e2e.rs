//! End-to-end pass over the window in demo mode, as the app runs it: the monitor and task threads collect in the
//! background from a machine whose every access is slowed down, while the test clicks every button, checkbox,
//! radio button, tab and drop-down of every page (and every Logs and Setup tab and onboarding step), types into the filters,
//! answers the dialogs and resizes the window.
//!
//! Asserts: the UI thread never touched the machine; no frame took longer than the budget (50 ms in release builds);
//! no page panicked; nothing was started (demo + dry run).
mod common;

use common::Ui;
use eframe::egui;
use eframe::egui::accesskit::Role;
use omarchyvr_gui_core::app::Page;
use omarchyvr_gui_core::crash::{self, Kind};
use omarchyvr_app::app::{LogsTab, SetupTab};
use std::time::Duration;

/// The sidebar's entries (never clicked by the sweep of a page: they leave it). "Developer" also keeps the sweep off
/// the Developer mode switch, which would hide the advanced pages.
const NAV: [&str; 16] = [
    "Home", "Display", "Tracking", "Room", "Desktop", "Backgrounds", "Recording", "Local AI", "AI", "Ready", "Developer", "Tools", "Logs", "Profiles", "System",
    "Onboarding",
];

fn nav_label(p: Page) -> &'static str {
    omarchyvr_app::app::page_label(p)
}

/// Go to a page through the navigation.
fn go(ui: &mut Ui, p: Page) {
    let label = ui
        .widgets()
        .into_iter()
        .find(|(r, l)| *r == Role::Button && (l == nav_label(p) || l.starts_with(&format!("{} (", nav_label(p)))))
        .map(|(_, l)| l)
        .unwrap_or_else(|| panic!("no navigation button for {p:?}"));
    assert!(ui.click(Role::Button, &label, 0));
    assert_eq!(ui.h.state().core.page, p, "the navigation went to {p:?}");
}

fn all_pages(ui: &mut Ui) -> usize {
    let mut total = 0;
    for p in Page::ALL {
        go(ui, p);
        let mut skip: Vec<&str> = NAV.to_vec();
        match p {
            Page::Desk => {
                total += ui.click_everything(&skip);
                // a mock headset in the picker
                ui.h.state_mut().ui.model = Some("8kx");
                ui.frames(3, Duration::from_millis(10), "mock headset");
                ui.h.state_mut().ui.model = None;
            }
            Page::Logs => {
                skip.extend(["Logs", "Problems and fixes", "Open issues", "Lessons", "Safety rules"]);
                for tab in [LogsTab::Logs, LogsTab::History, LogsTab::Issues, LogsTab::Lessons, LogsTab::Safety] {
                    ui.h.state_mut().ui.logs_tab = tab;
                    // the data comes from the background: frames go on meanwhile
                    let ok = ui.until(Duration::from_secs(20), "logs data", |a| match tab {
                        LogsTab::Logs => a.core.logs.listed && (a.core.logs.sources.is_empty() || a.core.logs.tail.is_some()),
                        _ => !a.core.task_busy("docs"),
                    });
                    assert!(ok, "{tab:?} data did not arrive");
                    ui.frames(5, Duration::from_millis(20), "logs tab");
                    total += ui.click_everything(&skip);
                }
            }
            Page::Setup => {
                skip.extend(["Checks", "Install", "Autofix"]);
                for tab in [SetupTab::Checks, SetupTab::Install, SetupTab::Autofix] {
                    ui.h.state_mut().ui.setup_tab = tab;
                    // the Autofix bundle is collected in the background: frames go on meanwhile
                    let ok = ui.until(Duration::from_secs(20), "setup tab data", |a| match tab {
                        SetupTab::Autofix => a.core.autofix.preview.is_some() && a.core.ai.loaded,
                        _ => a.core.facts.findings.is_some(),
                    });
                    assert!(ok, "{tab:?} data did not arrive");
                    ui.frames(3, Duration::from_millis(10), "setup tab");
                    total += ui.click_everything(&skip);
                    if tab == SetupTab::Autofix {
                        // the demo answer (nothing is sent): its fix cards, approve and decline included
                        assert!(ui.until(Duration::from_secs(20), "autofix answer", |a| a.core.autofix.answer.is_some()), "no demo answer");
                        ui.frames(3, Duration::from_millis(10), "answer");
                        total += ui.click_everything(&skip);
                    }
                }
            }
            Page::Ai => {
                // the providers stay (Remove is covered by the core's tests)
                skip.push("Remove");
                assert!(ui.until(Duration::from_secs(20), "ai settings", |a| a.core.ai.loaded && a.core.ai.probed), "the AI settings did not arrive");
                ui.frames(3, Duration::from_millis(10), "ai page");
                total += ui.click_everything(&skip);
                // the editor, open
                let claude = ui.h.state().core.ai.config.provider("claude").cloned().expect("demo provider");
                ui.h.state_mut().ui.ai_draft = Some(omarchyvr_app::pages::ai::AiDraft::for_shot(&claude));
                ui.frames(3, Duration::from_millis(10), "ai editor");
                total += ui.click_everything(&skip);
            }
            Page::System => {
                assert!(ui.until(Duration::from_secs(20), "omarchyvrd state", |a| a.core.system.snap.is_some()), "no System data");
                ui.frames(3, Duration::from_millis(10), "system page");
                total += ui.click_everything(&skip);
            }
            Page::Onboarding => {
                let steps = ui.h.state().core.onboarding_steps();
                let titles: Vec<&str> = steps.iter().map(|s| s.title()).collect();
                skip.extend(titles.iter().copied());
                for s in steps {
                    ui.h.state_mut().core.onboarding.view = Some(s);
                    ui.step("onboarding step");
                    total += ui.click_everything(&skip);
                    ui.h.state_mut().core.page = Page::Onboarding;
                }
                // the stepper itself
                ui.h.state_mut().core.onboarding.view = None;
                ui.step("onboarding start");
                total += ui.click_everything(&NAV);
                ui.h.state_mut().core.page = Page::Onboarding;
            }
            _ => total += ui.click_everything(&skip),
        }
        ui.frames(3, Duration::from_millis(10), "page done");
        for (w, h) in [(760.0, 520.0), (1920.0, 1200.0), (1000.0, 700.0), (1280.0, 900.0)] {
            ui.resize(w, h);
        }
        assert!(ui.h.state().ui.page_error.is_none(), "{p:?} failed: {:?}", ui.h.state().ui.page_error);
        eprintln!("{p:?}: {total} clicks so far");
    }
    total
}

#[test]
fn every_page_and_widget_in_demo_mode_stays_responsive() {
    let mut ui = Ui::new("e2e", false, Duration::from_millis(5));
    // the window draws at once; the first check of the machine runs in the background
    ui.frames(3, Duration::ZERO, "first frames");
    assert_eq!(ui.h.state().core.facts.at, 0, "nothing was collected on the UI thread");
    assert!(ui.h.state().core.preflight().get("checking").is_some(), "the desk page says it is still checking");
    ui.wait_facts();
    // the launcher plans of the desk and the VR start are worked out in the background, then both preflights judge
    assert!(
        ui.until(Duration::from_secs(10), "plans", |a| a.core.preflight().get("checking").is_none() && a.core.vr_preflight().get("checking").is_none()),
        "a preflight stayed at checking"
    );
    let clicks = all_pages(&mut ui);
    assert!(clicks > 100, "only {clicks} widgets were clicked");
    // the report dialog (the Developer page) writes a real (temporary) report in demo mode
    go(&mut ui, Page::Developer);
    ui.frames(3, Duration::from_millis(15), "developer page");
    assert!(ui.click(Role::Button, "Report a problem...", 0), "{:?}", ui.widgets());
    assert!(ui.h.state().ui.report_open);
    assert!(ui.click(Role::Button, "Create report", 0));
    assert!(ui.until(Duration::from_secs(20), "report", |a| a.core.problem.result.is_some()), "the report was not written");
    let r = ui.h.state().core.problem.result.clone().unwrap();
    let path = r.expect("report written");
    assert!(path.starts_with(&ui.dir), "{}", path.display());
    ui.close_dialogs();
    let a = ui.h.state();
    assert!(a.core.job.is_none(), "demo mode started a job");
    let p = common::app_panics();
    assert!(p.is_empty(), "panics in the app: {p:?}");
    ui.assert_responsive();
    eprintln!("{} clicks in total", ui.clicked);
    let _ = std::fs::remove_dir_all(&ui.dir);
}

#[test]
fn running_session_and_the_crash_dialog() {
    let mut ui = Ui::new("e2e-run", true, Duration::from_millis(5));
    ui.wait_facts();
    assert!(ui.until(Duration::from_secs(20), "live", |a| a.core.live.service.is_some()), "the demo session shows");
    // a crash report from an earlier run: the dialog lists what still runs and offers the report
    let state = ui.h.state().core.paths.state_dir.clone();
    let mut r = crash::Report::new(Kind::Native, "the window process died from SIGSEGV (invalid memory access)", "signal 11");
    r.context = vec![("job".into(), "Start desk mode (--fps 72 --6dof)".into())];
    crash::write(&state, &r).unwrap();
    ui.h.state_mut().core.check_crashes();
    assert!(ui.until(Duration::from_secs(10), "crash dialog", |a| a.core.recovery.is_some()), "no crash dialog");
    // a dialog lays itself out invisibly in its first frame
    ui.frames(2, Duration::ZERO, "crash dialog shown");
    let orphans = ui.h.state().core.orphans();
    assert!(orphans.iter().any(|o| o.text.starts_with("Desk mode is still running")), "{orphans:?}");
    assert!(orphans.iter().any(|o| o.text.contains("Start desk mode")), "{orphans:?}");
    for label in ["Copy report", "Open folder", "The report"] {
        assert!(ui.click(Role::Button, label, 0), "{label} not in {:?}", ui.widgets());
    }
    assert!(ui.click(Role::Button, "Stop session", 0), "the stop action of the dialog");
    assert!(ui.click(Role::Button, "Close", 0));
    assert!(ui.h.state().core.recovery.is_none());
    assert!(ui.until(Duration::from_secs(5), "seen", |_| crash::unseen(&state).is_empty()), "the report counts as seen");
    let clicks = all_pages(&mut ui);
    assert!(clicks > 80, "only {clicks} widgets were clicked");
    assert!(ui.h.state().core.job.is_none(), "demo mode started a job");
    ui.assert_responsive();
    let _ = std::fs::remove_dir_all(&ui.dir);
}

/// The "?" opens its text on a click and closes on a second click or Esc (hover is unreliable in the headset).
#[test]
fn the_question_mark_opens_on_click_and_closes_on_click_or_esc() {
    let mut ui = Ui::new("help-click", false, std::time::Duration::ZERO);
    ui.wait_facts();
    go(&mut ui, Page::Ai);
    ui.frames(4, std::time::Duration::from_millis(10), "local ai");
    let open = |ui: &Ui| egui::Popup::is_any_open(&ui.h.ctx);
    assert!(!open(&ui));
    assert!(ui.click(Role::Button, "Help", 1), "the top bar has the first Help, the \"?\" marks follow");
    ui.frames(1, std::time::Duration::from_millis(10), "popup");
    assert!(open(&ui), "a click opens the popup");
    assert!(ui.click(Role::Button, "Help", 1));
    ui.frames(3, std::time::Duration::from_millis(10), "popup closed");
    assert!(!open(&ui), "a second click closes it");
    assert!(ui.click(Role::Button, "Help", 1));
    ui.frames(3, std::time::Duration::from_millis(10), "popup");
    assert!(open(&ui));
    ui.h.key_press(egui::Key::Escape);
    ui.frames(3, std::time::Duration::from_millis(10), "esc");
    assert!(!open(&ui), "Esc closes it");
}
