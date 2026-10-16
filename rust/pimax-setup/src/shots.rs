//! `pimax-setup --screenshots DIR`: every page rendered headlessly on the CPU ([`crate::screenshot`]) from the demo
//! fixtures of `pimax_gui_core::demo` (no real machine state, no window, no GPU, nothing started).
use crate::app::{LogsTab, PimaxApp, SetupTab};
use crate::screenshot;
use eframe::egui;
use pimax_config::Store;
use pimax_gui_core::app::{AppCore, Page, Pending};
use pimax_gui_core::checklist::Fix;
use pimax_gui_core::device::onboarding::{State, Step};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy)]
struct Shot {
    name: &'static str,
    running: bool,
    page: Page,
    height: f32,
    dark: bool,
    setup: fn(&mut PimaxApp),
}

fn shots() -> Vec<Shot> {
    vec![
        Shot { name: "desk-idle", running: false, page: Page::Desk, height: 1080.0, dark: true, setup: |_| {} },
        Shot { name: "desk-running", running: true, page: Page::Desk, height: 980.0, dark: true, setup: |_| {} },
        Shot { name: "desk-idle-light", running: false, page: Page::Desk, height: 1080.0, dark: false, setup: |_| {} },
        Shot {
            name: "home-rows",
            running: true,
            page: Page::Desk,
            height: 1500.0,
            dark: true,
            setup: |a| {
                a.ui.open_rows = vec!["row-quality", "row-desktop"];
            },
        },
        Shot {
            name: "home-optics",
            running: false,
            page: Page::Desk,
            height: 1250.0,
            dark: true,
            setup: |a| {
                // IPD: lenses read from the headset, a value being edited, the last move's result; the headset card shows
                // the last restart's result
                a.core.lens_ipd = Some(61.4);
                a.core.lens_ipd_asked = true;
                a.ui.ipd_edit = Some(58.0);
                a.core.ipd_result = Some(pimax_gui_core::ipdmotor::IpdResult { ok: true, text: "Lenses set to 61.4 mm.".into() });
                a.core.headset_reset = Some(pimax_gui_core::reset::ResetResult { ok: true, text: "Headset restarted: the camera is back after 17 s.".into() });
                a.ui.open_rows = vec!["row-optics"];
            },
        },
        Shot {
            name: "home-optics-running",
            running: true,
            page: Page::Desk,
            height: 1250.0,
            dark: true,
            setup: |a| {
                // a session runs: the lenses do not move and the page says why
                a.core.lens_ipd_asked = true;
                a.ui.ipd_edit = Some(60.0);
                a.ui.open_rows = vec!["row-optics"];
            },
        },
        Shot {
            name: "home-play-cap",
            running: false,
            page: Page::Desk,
            height: 1250.0,
            dark: true,
            setup: |a| {
                // the relink dropped cap_sys_nice on the build Play uses: the readiness line offers Fix
                for b in a.core.facts.builds.iter_mut().filter(|b| b.build.key == "desk") {
                    b.sys_nice = Some(false);
                }
                a.core.facts.polkit_agent = true;
                a.core.state.autostart = true;
                a.core.play.autostart_note = Some((false, "Not started at login: monado-service lacks cap_sys_nice (press Fix, or Play or Work once; no password prompt at login).".into()));
                a.ui.open_rows = vec!["row-tracking"];
            },
        },
        Shot {
            name: "home-room",
            running: true,
            page: Page::Desk,
            height: 1250.0,
            dark: true,
            setup: |a| {
                a.ui.open_rows = vec!["row-room", "row-voice"];
                a.ui.model = Some("crystal-super");
            },
        },
        Shot {
            name: "home-auto",
            running: true,
            page: Page::Desk,
            height: 1300.0,
            dark: true,
            setup: |a| {
                a.ui.open_rows = vec!["row-auto", "row-runtime"];
            },
        },
        Shot {
            name: "setup",
            running: false,
            page: Page::Setup,
            height: 1500.0,
            dark: true,
            setup: |a| a.core.setup_focus = Some("setcap-slam".into()),
        },
        Shot {
            name: "setup-pkexec",
            running: false,
            page: Page::Setup,
            height: 900.0,
            dark: true,
            setup: |a| {
                if let Some(it) = a.core.items.iter().find(|i| i.id == "setcap-slam") {
                    if let Fix::Root(r) = &it.fix {
                        a.core.pending = Some(Pending::Root { title: it.title.clone(), fix: r.clone(), catalogue: None });
                    }
                }
            },
        },
        Shot { name: "tools", running: true, page: Page::Tools, height: 2100.0, dark: true, setup: |_| {} },
        Shot {
            name: "logs",
            running: true,
            page: Page::Logs,
            height: 900.0,
            dark: true,
            setup: |a| a.ui.logs_tab = LogsTab::Logs,
        },
        Shot {
            name: "history",
            running: false,
            page: Page::Logs,
            height: 1100.0,
            dark: true,
            setup: |a| a.ui.logs_tab = LogsTab::History,
        },
        Shot {
            name: "lessons",
            running: false,
            page: Page::Logs,
            height: 1500.0,
            dark: true,
            setup: |a| a.ui.logs_tab = LogsTab::Lessons,
        },
        Shot {
            name: "setup-checks",
            running: false,
            page: Page::Setup,
            height: 1650.0,
            dark: true,
            setup: |a| {
                a.ui.setup_tab = SetupTab::Checks;
                a.core.want_fixes();
            },
        },
        Shot {
            name: "autofix",
            running: false,
            page: Page::Setup,
            height: 1700.0,
            dark: true,
            setup: |a| {
                a.ui.setup_tab = SetupTab::Autofix;
                a.core.want_autofix();
                a.core.autofix_send();
            },
        },
        Shot {
            name: "ai",
            running: false,
            page: Page::Ai,
            height: 2150.0,
            dark: true,
            setup: |a| {
                a.core.want_ai();
                a.core.ai_test("claude");
                a.core.ai_test("openrouter");
            },
        },
        Shot {
            name: "ai-edit",
            running: false,
            page: Page::Ai,
            height: 1300.0,
            dark: true,
            setup: |a| {
                a.core.want_ai();
                let p = a.core.ai.config.provider("claude").cloned();
                if let Some(p) = p {
                    a.ui.ai_draft = Some(crate::pages::ai::AiDraft::for_shot(&p));
                }
            },
        },
        Shot {
            name: "ai-edit-openrouter",
            running: false,
            page: Page::Ai,
            height: 2700.0,
            dark: true,
            setup: |a| {
                a.core.want_ai();
                let p = a.core.ai.config.provider("openrouter").cloned();
                if let Some(p) = p {
                    a.ui.ai_draft = Some(crate::pages::ai::AiDraft::for_shot(&p));
                }
            },
        },
        Shot {
            name: "system",
            running: true,
            page: Page::System,
            height: 2000.0,
            dark: true,
            setup: |a| a.core.want_system(),
        },
        Shot {
            name: "onboarding",
            running: false,
            page: Page::Onboarding,
            height: 1000.0,
            dark: true,
            setup: |a| a.core.want_onboarding(),
        },
        Shot {
            name: "onboarding-pair",
            running: true,
            page: Page::Onboarding,
            height: 900.0,
            dark: true,
            setup: |a| {
                a.core.want_onboarding();
                a.core.onboarding_use_module("dream-air");
                for s in [Step::Checks, Step::Fixes, Step::Udev, Step::Ipd] {
                    a.core.onboarding_set(s, State::Done);
                }
                a.core.pair("left");
            },
        },
        Shot {
            name: "profiles",
            running: false,
            page: Page::Profiles,
            height: 760.0,
            dark: true,
            setup: |a| a.core.profiles.import_legacy(),
        },
    ]
}

/// Render every page of an existing core (the real machine: `--screenshots DIR --real`, for bug reports). Only the
/// page and the Logs tab change between pictures; nothing is clicked, written or started.
pub fn render_core(dir: &Path, core: AppCore) -> io::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir)?;
    let mut app = PimaxApp::new(core);
    let mut out = Vec::new();
    for (name, page, tab, height) in [
        ("desk", Page::Desk, LogsTab::Logs, 1000.0),
        ("setup", Page::Setup, LogsTab::Logs, 1600.0),
        ("tools", Page::Tools, LogsTab::Logs, 2100.0),
        ("logs", Page::Logs, LogsTab::Logs, 900.0),
        ("issues", Page::Logs, LogsTab::Issues, 1000.0),
        ("onboarding", Page::Onboarding, LogsTab::Logs, 1000.0),
        ("profiles", Page::Profiles, LogsTab::Logs, 800.0),
    ] {
        app.core.page = page;
        app.ui.logs_tab = tab;
        app.ui.page_t0 = Some((page, f64::NEG_INFINITY));
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, true);
        let path = dir.join(format!("{name}.png"));
        screenshot::snapshot(&ctx, egui::vec2(1280.0, height), 1.0, 3, |ui| app.draw(ui), &path)?;
        out.push(path);
    }
    Ok(out)
}

/// Render all pages into `dir`; returns the files written.
pub fn render_all(dir: &Path) -> io::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir)?;
    let tmp = std::env::temp_dir().join(format!("pimax-setup-shots-{}", std::process::id()));
    let mut out = Vec::new();
    // every picture twice: the simple flow (Developer mode off) and the same page with Developer mode on (name-dev.png)
    let jobs: Vec<(Shot, bool)> = shots().into_iter().flat_map(|s| [(s, false), (s, true)]).collect();
    for (s, dev) in jobs {
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("cfg"))?;
        std::fs::write(tmp.join("cfg/lens.conf"), "# headset serial SER1\ncant_deg = 2.0\n[left]\ncenter = 0.0271 0.009\n[right]\ncenter = -0.0271 0.009\n")?;
        // a fixed clock: the backup names in the pictures do not change between runs
        let store = Store::with_clock(tmp.join("cfg"), tmp.join("cache"), || 1_791_968_155);
        let mut core = AppCore::demo(s.running, store);
        core.paths.state_dir = tmp.join("state");
        core.page = s.page;
        core.state.dev_mode = dev;
        let mut app = PimaxApp::new(core);
        // pictures show the page as it settles (no fade-in)
        app.ui.page_t0 = Some((s.page, f64::NEG_INFINITY));
        (s.setup)(&mut app);
        let ctx = egui::Context::default();
        crate::theme::install(&ctx, s.dark);
        let path = dir.join(if dev { format!("{}-dev.png", s.name) } else { format!("{}.png", s.name) });
        screenshot::snapshot(&ctx, egui::vec2(1280.0, s.height), 1.0, 3, |ui| app.draw(ui), &path)?;
        out.push(path);
    }
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(out)
}
