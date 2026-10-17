//! `omarchyvr-app --screenshots DIR`: every page rendered headlessly on the CPU ([`crate::screenshot`]) from the demo
//! fixtures of `omarchyvr_gui_core::demo` (no real machine state, no window, no GPU, nothing started).
use crate::app::{LogsTab, PimaxApp, SetupTab};
use crate::screenshot;
use eframe::egui;
use omarchyvr_config::Store;
use omarchyvr_gui_core::app::{AppCore, Page, Pending};
use omarchyvr_gui_core::checklist::Fix;
use omarchyvr_gui_core::device::onboarding::{State, Step};
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
        Shot { name: "desk-idle", running: false, page: Page::Desk, height: 820.0, dark: true, setup: |_| {} },
        Shot {
            name: "desk-running",
            running: true,
            page: Page::Desk,
            height: 820.0,
            dark: true,
            setup: |app| {
                // the running session's in-hand status (PIMAX_CTRL_INHAND): the right controller is in the hand, the left one sends nothing
                let side = |state: &str, why: &str| omarchyvr_config::ctrl_inhand::Side { state: state.into(), why: why.into() };
                app.core.live.ctrl_inhand = Some(omarchyvr_config::ctrl_inhand::Status {
                    pid: 1,
                    mode: omarchyvr_config::ctrl_inhand::CtrlMode::WhenHeld,
                    left: side("down", "no reports"),
                    right: side("held", "motion"),
                });
            },
        },
        Shot { name: "desk-idle-light", running: false, page: Page::Desk, height: 820.0, dark: false, setup: |_| {} },
        Shot {
            name: "home-play-cap",
            running: false,
            page: Page::Desk,
            height: 860.0,
            dark: true,
            setup: |a| {
                // the relink dropped cap_sys_nice on the build Play uses: the readiness line offers Fix
                for b in a.core.facts.builds.iter_mut().filter(|b| b.build.key == "desk") {
                    b.sys_nice = Some(false);
                }
                a.core.facts.polkit_agent = true;
                a.core.state.autostart = true;
                a.core.play.autostart_note = Some((false, "Not started at login: monado-service lacks cap_sys_nice (press Fix, or Play or Work once; no password prompt at login).".into()));
                // the last headset restart's result under the headset card
                a.core.headset_reset = Some(omarchyvr_gui_core::reset::ResetResult { ok: true, text: "Headset restarted: the camera is back after 17 s.".into() });
            },
        },
        Shot {
            name: "home-mock",
            running: false,
            page: Page::Desk,
            height: 820.0,
            dark: true,
            setup: |a| {
                // a headset module that is listed but not supported yet
                a.ui.model = Some("crystal-super");
            },
        },
        Shot {
            name: "display",
            running: false,
            page: Page::Display,
            height: 820.0,
            dark: true,
            setup: |a| {
                // IPD: lenses read from the headset, a value being edited, the last move's result
                a.core.lens_ipd = Some(61.4);
                a.core.lens_ipd_asked = true;
                a.ui.ipd_edit = Some(58.0);
                a.core.ipd_result = Some(omarchyvr_gui_core::ipdmotor::IpdResult { ok: true, text: "Lenses set to 61.4 mm.".into() });
            },
        },
        Shot {
            name: "display-running",
            running: true,
            page: Page::Display,
            height: 820.0,
            dark: true,
            setup: |a| {
                // a session runs: the lenses do not move and the page says why
                a.core.lens_ipd_asked = true;
                a.ui.ipd_edit = Some(60.0);
            },
        },
        Shot { name: "tracking", running: true, page: Page::Tracking, height: 900.0, dark: true, setup: |_| {} },
        Shot {
            name: "tracking-handmodels",
            running: false,
            page: Page::Tracking,
            height: 900.0,
            dark: true,
            setup: |a| {
                // the models are not downloaded yet; the consent notice is on screen (Developer mode: -dev picture)
                a.core.facts.hands_models = false;
                a.core.hand_models_ask();
            },
        },
        Shot {
            name: "tracking-handmodels-error",
            running: false,
            page: Page::Tracking,
            height: 900.0,
            dark: true,
            setup: |a| {
                a.core.facts.hands_models = false;
                a.core.hand_models.result = Some(Err("git-lfs is not installed on this PC, and the hand models need it. Install the package \"git-lfs\", then press Download again. Nothing was changed.".into()));
            },
        },
        Shot { name: "room", running: true, page: Page::Room, height: 900.0, dark: true, setup: |_| {} },
        Shot { name: "desktop", running: true, page: Page::Desktop, height: 820.0, dark: true, setup: |_| {} },
        Shot { name: "developer", running: true, page: Page::Developer, height: 1300.0, dark: true, setup: |_| {} },
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
        Shot {
            name: "restart-steam",
            running: false,
            page: Page::Display,
            height: 700.0,
            dark: true,
            setup: |a| {
                let steam = omarchyvr_gui_core::facts::TaintedSteam { pid: 16972, vars: vec!["SteamAppId".into(), "PIMAX_SLAM".into()], busy: Some("a game seems to be running".into()) };
                a.core.pending = Some(Pending::RestartSteam { steam, vr: true });
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
            height: 1000.0,
            dark: true,
            setup: |a| {
                // the default: Claude (CLI) found, model Haiku, a good test
                a.core.want_ai();
                a.core.ai_set_voice_brain(omarchyvr_gui_core::ai::VoiceBrain::Claude);
                a.core.ai_set_claude_model("haiku");
                a.core.claude_look();
                a.core.claude_test_run();
            },
        },
        Shot {
            name: "ai-claude-missing",
            running: false,
            page: Page::Ai,
            height: 1000.0,
            dark: true,
            setup: |a| {
                // Claude Code not installed: the plain hint, Test off
                a.core.want_ai();
                a.core.ai_set_voice_brain(omarchyvr_gui_core::ai::VoiceBrain::Claude);
                a.core.ai.claude_status = Some(omarchyvr_config::claude_cli::ClaudeCliStatus::NotFound);
            },
        },
        Shot {
            name: "ai-claude-login",
            running: false,
            page: Page::Ai,
            height: 1000.0,
            dark: true,
            setup: |a| {
                // found, but the test says the login is missing: the Open Claude login button
                a.core.want_ai();
                a.core.ai_set_voice_brain(omarchyvr_gui_core::ai::VoiceBrain::Claude);
                a.core.ai_set_claude_model("sonnet");
                a.core.claude_look();
                a.core.ai.claude_test = Some(Err("Claude CLI is not logged in".into()));
            },
        },
        Shot {
            name: "ai-local",
            running: true,
            page: Page::Ai,
            height: 1800.0,
            dark: true,
            setup: |a| {
                a.core.want_ai();
                a.core.want_localai();
                a.core.claude_look();
                a.core.ai_set_local_models(true, false);
                a.core.localai.test = Some(Ok(omarchyvr_gui_core::localai::parse_checks(
                    &[
                        "check router ok 31 Lara sorted a test command: system",
                        "check action ok 190 Qwen3.5-4B-Q4_K_M (gpu (all layers)); 3.4 GB of video memory, 19.3 GB free",
                        "check speech skip 0 large model off, the small one answers",
                        "check agent fail 412 Qwen3.6-27B: port 8083: connection refused",
                    ]
                    .map(String::from),
                )));
            },
        },
        Shot {
            name: "ai-help",
            running: true,
            page: Page::Ai,
            height: 900.0,
            dark: true,
            setup: |a| {
                a.core.want_ai();
                a.core.claude_look();
            },
        },
        Shot {
            name: "ai-online",
            running: false,
            page: Page::Ai,
            height: 1700.0,
            dark: true,
            setup: |a| {
                // Online with a key (OpenRouter in the demo file): the providers and which one does what
                a.core.want_ai();
                a.core.claude_look();
                a.core.ai_test("claude");
                a.core.ai_test("openrouter");
            },
        },
        Shot {
            name: "ai-edit-cli",
            running: false,
            page: Page::Ai,
            height: 2300.0,
            dark: true,
            setup: |a| {
                a.core.want_ai();
                a.core.claude_look();
                let mut p = omarchyvr_config::ai::Provider::claude_cli();
                p.command = Some("~/.local/bin/claude".into());
                a.ui.ai_draft = Some(crate::pages::ai::AiDraft::for_shot(&p));
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
            name: "onboarding-system",
            running: false,
            page: Page::Onboarding,
            height: 900.0,
            dark: true,
            setup: |a| {
                a.core.want_onboarding();
                a.core.onboarding_use_module("dream-air");
                for s in [Step::Checks, Step::Fixes] {
                    a.core.onboarding_set(s, State::Done);
                }
                a.core.onboarding.install.root_done = false;
            },
        },
        Shot {
            name: "onboarding-nvidia-untested",
            running: false,
            page: Page::Onboarding,
            height: 1300.0,
            dark: true,
            setup: |a| {
                a.core.want_onboarding();
                a.core.onboarding_use_module("dream-air");
                for s in [Step::Checks, Step::Fixes] {
                    a.core.onboarding_set(s, State::Done);
                }
                // an untested driver, patched because the user allowed it (relaxed matching), the switch just turned off
                a.core.onboarding.install.nvidia = omarchyvr_gui_core::nvidia_patch::NvidiaInfo {
                    driver: Some("610.58.01".into()),
                    tested: Some(false),
                    tested_list: vec!["610.57.04".into()],
                    status: omarchyvr_gui_core::nvidia_patch::PatchStatus::parse("driver=610.58.01\nresult=untested-allowed\nfuzz=2\ntime=2026-10-17T08:00:00Z\n"),
                    conf_allow: Some(true),
                };
                a.core.state.nvidia_untested = false;
            },
        },
        Shot {
            name: "uninstall",
            running: false,
            page: Page::Setup,
            height: 900.0,
            dark: true,
            setup: |a| a.core.ask_uninstall_demo(),
        },
        Shot {
            name: "profiles",
            running: false,
            page: Page::Profiles,
            height: 760.0,
            dark: true,
            setup: |a| a.core.profiles.import_legacy(),
        },
        // Play running: the graphics card is the game's (the check while Play runs), what Play stopped, Switch to Work
        Shot {
            name: "home-play-running",
            running: true,
            page: Page::Desk,
            height: 860.0,
            dark: true,
            setup: |a| {
                a.core.live.started_as = Some("vr".into());
                for p in a.core.live.session.iter_mut() {
                    p.alive = matches!(p.name, "monado" | "broker" | "ctrltrack");
                }
                a.core.live.gm_paused = vec!["local AI".into(), "voice".into()];
                a.core.vram = Some((std::time::Instant::now(), omarchyvr_gui_core::vram::demo_report()));
            },
        },
        // the Work first run (docs/WORK-FIRST-RUN.md): the first Work press asks two questions before VR starts
        Shot {
            name: "worksetup-assistant",
            running: false,
            page: Page::WorkSetup,
            height: 1180.0,
            dark: true,
            setup: |a| {
                a.core.work_setup.record.done = false;
                a.core.open_work_setup(true);
                a.core.work_setup.choices = omarchyvr_gui_core::worksetup::Choices::default();
            },
        },
        Shot {
            name: "worksetup-assistant-local",
            running: false,
            page: Page::WorkSetup,
            height: 1180.0,
            dark: true,
            setup: |a| {
                use omarchyvr_gui_core::worksetup::{Brain, Choices};
                a.core.work_setup.record.done = false;
                a.core.open_work_setup(true);
                a.core.work_setup.choices = Choices { brain: Brain::Local, lara: true, ..Choices::default() };
            },
        },
        Shot {
            name: "worksetup-assistant-api",
            running: false,
            page: Page::WorkSetup,
            height: 1240.0,
            dark: true,
            setup: |a| {
                use omarchyvr_gui_core::worksetup::{Brain, Choices};
                a.core.work_setup.record.done = false;
                a.core.open_work_setup(true);
                a.core.work_setup.choices = Choices { brain: Brain::Compatible, lara: true, base_url: "https://openrouter.ai/api/v1".into(), model: "anthropic/claude-haiku-5.5".into(), ..Choices::default() };
                a.core.work_setup.key = "sk-or-v1-demo-not-a-key".into();
                a.core.work_setup.test = Some(Ok("OK: anthropic/claude-haiku-5.5 answered in 0.9 s".into()));
            },
        },
        Shot {
            name: "worksetup-speech",
            running: false,
            page: Page::WorkSetup,
            height: 900.0,
            dark: true,
            setup: |a| {
                use omarchyvr_gui_core::worksetup::{Brain, Choices, Step};
                a.core.work_setup.record.done = false;
                a.core.open_work_setup(true);
                a.core.work_setup.choices = Choices { brain: Brain::Local, lara: true, ..Choices::default() };
                a.core.work_setup.step = Step::Speech;
            },
        },
        Shot {
            name: "worksetup-download",
            running: false,
            page: Page::WorkSetup,
            height: 900.0,
            dark: true,
            setup: |a| {
                use omarchyvr_gui_core::worksetup::{Brain, Choices};
                a.core.work_setup.record.done = false;
                a.core.open_work_setup(true);
                a.core.work_setup.choices = Choices { brain: Brain::Local, lara: true, ..Choices::default() };
                a.core.ws_next();
                a.core.ws_finish();
            },
        },
        Shot {
            name: "worksetup-download-error",
            running: false,
            page: Page::WorkSetup,
            height: 900.0,
            dark: true,
            setup: |a| {
                use omarchyvr_gui_core::worksetup::{apply_line, Brain, Choices};
                a.core.work_setup.record.done = false;
                a.core.open_work_setup(true);
                a.core.work_setup.choices = Choices { brain: Brain::ClaudeKey, lara: true, ..Choices::default() };
                a.core.ws_next();
                a.core.ws_finish();
                if let Some(st) = a.core.work_setup.get.clone() {
                    if let Ok(mut s) = st.lock() {
                        apply_line(&mut s, "get error lara downloading the Laya checkpoint convaiinnovations/laya at 55cf4c4 failed (exit status: 1); press Download again to retry (it resumes)");
                    }
                }
                a.core.poll();
            },
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
        ("desk", Page::Desk, LogsTab::Logs, 860.0),
        ("display", Page::Display, LogsTab::Logs, 860.0),
        ("tracking", Page::Tracking, LogsTab::Logs, 900.0),
        ("room", Page::Room, LogsTab::Logs, 900.0),
        ("desktop", Page::Desktop, LogsTab::Logs, 860.0),
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
        screenshot::snapshot(&ctx, egui::vec2(shot_width(), shot_height(height)), 1.0, 3, |ui| app.draw(ui), &path)?;
        out.push(path);
    }
    Ok(out)
}

/// Render all pages into `dir`; returns the files written.
pub fn render_all(dir: &Path) -> io::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir)?;
    let tmp = std::env::temp_dir().join(format!("omarchyvr-setup-shots-{}", std::process::id()));
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
        if s.name == "ai-help" {
            crate::widgets::open_next_help(&ctx);
        }
        let path = dir.join(if dev { format!("{}-dev.png", s.name) } else { format!("{}.png", s.name) });
        screenshot::snapshot(&ctx, egui::vec2(shot_width(), shot_height(s.height)), 1.0, 3, |ui| app.draw(ui), &path)?;
        out.push(path);
    }
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(out)
}

/// The screenshot width in points: 1280, or `PIMAX_SHOT_WIDTH` (to check the minimum window size).
fn shot_width() -> f32 {
    std::env::var("PIMAX_SHOT_WIDTH").ok().and_then(|v| v.parse().ok()).unwrap_or(1280.0)
}

/// The screenshot height in points: the picture's own, or `PIMAX_SHOT_HEIGHT` for all of them (with
/// `PIMAX_SHOT_WIDTH=900 PIMAX_SHOT_HEIGHT=560`: every page at the window's minimum size).
fn shot_height(own: f32) -> f32 {
    std::env::var("PIMAX_SHOT_HEIGHT").ok().and_then(|v| v.parse().ok()).unwrap_or(own)
}
