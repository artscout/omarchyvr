//! The Work first run in the window (demo mode, the harness of e2e.rs): the first Work opens the questions; every
//! button, radio button and switch of each page (every brain, speech, the downloads) is clicked. Asserts: the UI thread
//! never touched the machine, no frame over the budget, no panic, nothing started (demo + dry run), Play never shows
//! the questions.
mod common;

use common::Ui;
use eframe::egui::accesskit::Role;
use omarchyvr_gui_core::app::Page;
use omarchyvr_gui_core::worksetup::{Brain, Choices, Speech, Step};
use std::time::Duration;

/// Never clicked by the sweep: the navigation (it leaves the page; the list of e2e.rs) and Close (it closes the
/// questions).
const SKIP: [&str; 17] = [
    "Home", "Display", "Tracking", "Room", "Desktop", "Backgrounds", "Recording", "Local AI", "AI", "Ready", "Developer", "Tools", "Logs", "Profiles", "System",
    "Onboarding", "Close",
];

#[test]
fn every_widget_of_the_work_questions_stays_responsive_and_starts_nothing() {
    let mut ui = Ui::new("worksetup", false, Duration::from_millis(5));
    ui.wait_facts();
    // Play never asks the Work questions
    ui.h.state_mut().core.work_setup.record.done = false;
    ui.h.state_mut().core.consent_force = true;
    ui.h.state_mut().core.play();
    ui.frames(2, Duration::from_millis(10), "play");
    assert_ne!(ui.h.state().core.page, Page::WorkSetup, "Play shows no Work questions");
    // the first Work does
    ui.h.state_mut().core.work();
    ui.frames(3, Duration::from_millis(10), "work questions");
    assert_eq!(ui.h.state().core.page, Page::WorkSetup);
    assert!(ui.until(Duration::from_secs(10), "ai settings", |a| a.core.ai.loaded), "the AI settings did not arrive");
    let mut total = 0;
    for brain in Brain::ALL {
        for lara in [false, true] {
            {
                let w = &mut ui.h.state_mut().core.work_setup;
                w.choices = Choices { brain, lara, ..Choices::default() };
                w.step = Step::Assistant;
                w.error = None;
            }
            ui.frames(2, Duration::from_millis(5), "assistant page");
            total += ui.click_everything(&SKIP);
            ui.h.state_mut().core.page = Page::WorkSetup;
        }
    }
    // the speech page with downloads to do, then the downloads (demo: nothing is fetched), Cancel, Try again
    for speech in Speech::ALL {
        {
            let w = &mut ui.h.state_mut().core.work_setup;
            w.choices = Choices { brain: Brain::Local, lara: true, speech, ..Choices::default() };
            w.step = Step::Speech;
            w.error = None;
            w.get = None;
        }
        ui.frames(2, Duration::from_millis(5), "speech page");
        total += ui.click_everything(&SKIP);
        ui.h.state_mut().core.page = Page::WorkSetup;
    }
    {
        let w = &mut ui.h.state_mut().core.work_setup;
        w.choices = Choices { brain: Brain::Local, lara: true, ..Choices::default() };
        w.step = Step::Speech;
    }
    ui.h.state_mut().core.ws_finish();
    ui.frames(2, Duration::from_millis(5), "downloads");
    assert_eq!(ui.h.state().core.work_setup.step, Step::Get);
    total += ui.click_everything(&SKIP);
    ui.frames(2, Duration::from_millis(5), "after cancel");
    total += ui.click_everything(&SKIP);
    for (w, h) in [(760.0, 520.0), (1920.0, 1200.0), (1280.0, 900.0)] {
        ui.resize(w, h);
    }
    assert!(total > 30, "only {total} widgets were clicked");
    assert!(ui.h.state().ui.page_error.is_none(), "the page failed: {:?}", ui.h.state().ui.page_error);
    assert!(ui.h.state().core.job.is_none(), "demo mode started a job");
    let p = common::app_panics();
    assert!(p.is_empty(), "panics in the app: {p:?}");
    ui.assert_responsive();
    // a first Work again, answered from the page: saved, then Work starts by itself (demo: "would run")
    {
        let core = &mut ui.h.state_mut().core;
        core.work_setup.record.done = false;
        core.work_setup.get = None;
        core.page = Page::Desktop;
        core.work();
        assert_eq!(core.page, Page::WorkSetup);
        core.work_setup.choices = Choices::default();
        core.work_setup.step = Step::Speech;
    }
    ui.frames(2, Duration::from_millis(5), "last page");
    assert!(ui.click(Role::Button, "Start Work", 0), "{:?}", ui.widgets());
    let core = &ui.h.state().core;
    assert!(core.work_setup_done());
    assert_eq!(core.page, Page::Desk);
    // Work was asked to start (the slowed demo machine may still be checking this PC: then it says so, nothing starts)
    let asked = core.message.starts_with("demo mode: would run Start desk mode") || core.message.starts_with("not starting: still checking");
    assert!(asked, "{}", core.message);
    assert!(core.job.is_none());
    // the next Work starts at once
    ui.h.state_mut().core.work();
    assert_ne!(ui.h.state().core.page, Page::WorkSetup);
    let _ = std::fs::remove_dir_all(&ui.dir);
}
