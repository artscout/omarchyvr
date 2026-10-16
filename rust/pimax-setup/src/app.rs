//! The window: navigation, pages, status bar with the job log, and the confirmation dialogs. All state and every
//! action live in [`pimax_gui_core::app::AppCore`].
use crate::pages;
use crate::theme::{self, icon};
use crate::widgets::{self, pal};
use eframe::egui::{self, RichText, Ui};
use pimax_gui_core::app::{AppCore, Page, Pending};
use pimax_gui_core::crash;
use pimax_gui_core::launcher::DeskToggles;
use pimax_gui_core::preflight::Preflight;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// Tabs of the Logs & History page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogsTab {
    /// Live log files.
    Logs,
    /// INVENTORY section 4.
    History,
    /// INVENTORY 5.1.
    Issues,
    /// LESSONS.md.
    Lessons,
    /// INVENTORY 6.
    Safety,
}

/// Tabs of the Setup page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupTab {
    /// The pimax-device checks with their fixes.
    Checks,
    /// The install checklist (doctor + LESSONS, builds, launcher, app).
    Install,
    /// Ask a language model (catalogue fixes only).
    Autofix,
}

/// UI-only state (what is open, filters, caches).
pub struct UiState {
    /// Job output panel open.
    pub show_log: bool,
    /// Logs page tab.
    pub logs_tab: LogsTab,
    /// Selected log.
    pub log_source: usize,
    /// Filter of the log view.
    pub log_filter: String,
    /// Re-read the log every second.
    pub log_follow: bool,
    /// Filter of the history tables.
    pub history_filter: String,
    /// Preflight cache (cleared when the switches or the facts change).
    pub pf: Option<(Instant, DeskToggles, u64, Preflight)>,
    /// Lines the user copied (feedback).
    pub copied: Option<Instant>,
    /// Setup item to highlight (id, since, scroll to it on the next frame).
    pub focus: Option<(String, Instant, bool)>,
    /// The "Report a problem" dialog is open.
    pub report_open: bool,
    /// A page that panicked while drawing: it shows an error card until "Try again".
    pub page_error: Option<(Page, String)>,
    /// Frames come at most this often (the window: 16 ms; tests: none).
    pub min_frame: Duration,
    /// When the last frame started.
    pub last_frame: Option<Instant>,
    /// Last slow-frame warning (rate limit).
    pub slow_warned: Option<Instant>,
    /// Frames drawn.
    pub frames: u64,
    /// IPD being edited (not applied yet).
    pub ipd_edit: Option<f32>,
    /// Backlight slider while it is dragged (percent).
    pub backlight_edit: Option<u8>,
    /// The headset model picked in the headset card (None: the connected one).
    pub model: Option<&'static str>,
    /// Scroll to the Room row on the next frame (ROOM SETUP).
    pub scroll_room: bool,
    /// The help dialog is open.
    pub help_open: bool,
    /// The page shown and when it was opened (for the fade-in).
    pub page_t0: Option<(Page, f64)>,
    /// Home rows to open on the next frame (screenshots, tests).
    pub open_rows: Vec<&'static str>,
    /// Setup page tab.
    pub setup_tab: SetupTab,
    /// The headset picked on the onboarding's first step (not used yet).
    pub onb_pick: Option<&'static str>,
    /// The AI provider being added or edited.
    pub ai_draft: Option<crate::pages::ai::AiDraft>,
}

impl Default for UiState {
    fn default() -> Self {
        UiState {
            show_log: false,
            logs_tab: LogsTab::Logs,
            log_source: 0,
            log_filter: String::new(),
            log_follow: true,
            history_filter: String::new(),
            pf: None,
            copied: None,
            focus: None,
            report_open: false,
            page_error: None,
            min_frame: Duration::ZERO,
            last_frame: None,
            slow_warned: None,
            frames: 0,
            ipd_edit: None,
            backlight_edit: None,
            model: None,
            scroll_room: false,
            help_open: false,
            page_t0: None,
            open_rows: Vec::new(),
            setup_tab: SetupTab::Checks,
            onb_pick: None,
            ai_draft: None,
        }
    }
}

/// The app.
pub struct PimaxApp {
    /// Core state.
    pub core: AppCore,
    /// UI state.
    pub ui: UiState,
}

impl PimaxApp {
    /// Wrap a core.
    pub fn new(core: AppCore) -> Self {
        PimaxApp { core, ui: UiState::default() }
    }

    /// The preflight, recomputed at most every 300 ms (it runs the launcher's option parser and the rules).
    pub fn preflight(&mut self) -> Preflight {
        let key = (self.core.toggles, self.core.facts.at);
        if let Some((t, tg, at, pf)) = &self.ui.pf {
            if t.elapsed() < Duration::from_millis(300) && (*tg, *at) == key && !self.core.busy() {
                return pf.clone();
            }
        }
        let pf = self.core.preflight();
        self.ui.pf = Some((Instant::now(), key.0, key.1, pf.clone()));
        pf
    }

    /// One frame of the window (what eframe calls `logic` + `ui`; the UI tests call this).
    pub fn frame(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        self.logic_step(&ctx);
        self.draw(ui);
    }

    /// Before drawing: the heartbeat, the frame limit, the background results.
    pub fn logic_step(&mut self, ctx: &egui::Context) {
        crash::beat();
        if ctx.data(|d| d.get_temp::<u64>(egui::Id::new("pimax-theme-installed"))).is_none() {
            // a context without the theme (tests): install it; it is complete from the next frame on
            theme::install(ctx, true);
            ctx.request_repaint();
        }
        if !self.ui.min_frame.is_zero() {
            // no vsync: never draw more than about 60 frames per second (animations, mouse moves)
            if let Some(t) = self.ui.last_frame {
                let e = t.elapsed();
                if e < self.ui.min_frame {
                    std::thread::sleep(self.ui.min_frame - e);
                }
            }
        }
        self.ui.last_frame = Some(Instant::now());
        self.ui.frames += 1;
        if crate::window::FOCUS.swap(false, Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
        }
        if self.core.poll() {
            self.ui.pf = None;
            ctx.request_repaint();
        }
        ctx.request_repaint_after(Duration::from_millis(1000));
    }

    /// Draw everything into the root ui. A page that panics shows an error card; the rest of the window goes on.
    pub fn draw(&mut self, ui: &mut Ui) {
        let t0 = Instant::now();
        let had_job = self.core.job.is_some();
        let p = theme::pal(ui);
        egui::Panel::top("top")
            .frame(egui::Frame::new().fill(p.bar).inner_margin(egui::Margin::symmetric(18, 10)).stroke(egui::Stroke::new(1.0, p.stroke)))
            .show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status")
            .frame(egui::Frame::new().fill(p.bar).inner_margin(egui::Margin::symmetric(16, 6)).stroke(egui::Stroke::new(1.0, p.stroke)))
            .show(ui, |ui| self.status_bar(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.bg).inner_margin(egui::Margin::symmetric(20, 16)))
            .show(ui, |ui| {
                let page = self.core.page;
                if page != Page::System {
                    // the System page's event stream stops a few seconds after it was left
                    self.core.system_closed();
                }
                // the new page fades in
                let now = ui.input(|i| i.time);
                let since = match self.ui.page_t0 {
                    Some((pg, t)) if pg == page => now - t,
                    _ => {
                        self.ui.page_t0 = Some((page, now));
                        0.0
                    }
                };
                let alpha = egui::emath::easing::cubic_out(((since / 0.16) as f32).clamp(0.0, 1.0));
                if alpha < 1.0 {
                    ui.multiply_opacity(0.25 + 0.75 * alpha);
                    ui.ctx().request_repaint();
                }
                match self.ui.page_error.clone() {
                    Some((p, msg)) if p == page => {
                        egui::ScrollArea::vertical().id_salt("page").auto_shrink([false, false]).show(ui, |ui| self.page_error_card(ui, p, &msg));
                    }
                    _ => {
                        let r = crash::catching(|| {
                            if page == Page::Desk {
                                // Home: two columns that scroll on their own
                                pages::home::show(ui, self);
                            } else {
                                egui::ScrollArea::vertical().id_salt("page").auto_shrink([false, false]).show(ui, |ui| {
                                    ui.spacing_mut().item_spacing.y = 10.0;
                                    match page {
                                        Page::Setup => pages::setup::show(ui, self),
                                        Page::Tools => pages::tools::show(ui, self),
                                        Page::Logs => pages::logs::show(ui, self),
                                        Page::Profiles => pages::profiles::show(ui, self),
                                        Page::Ai => pages::ai::show(ui, self),
                                        Page::System => pages::system::show(ui, self),
                                        Page::Onboarding => pages::onboarding::show(ui, self),
                                        Page::Desk => {}
                                    }
                                    ui.add_space(12.0);
                                });
                            }
                        });
                        if let Err(e) = r {
                            let msg = pimax_gui_core::worker::panic_text(e.as_ref());
                            log::error!("the {} page failed: {msg}", page.name());
                            self.ui.page_error = Some((page, msg));
                        }
                    }
                }
            });
        self.help_button(ui);
        self.modal(ui);
        self.recovery_dialog(ui);
        self.report_dialog(ui);
        self.help_dialog(ui);
        if !had_job && self.core.job.is_some() {
            // an action started: show its progress
            self.ui.show_log = true;
        }
        crate::window::FIRST_FRAME.store(true, Ordering::Relaxed);
        let took = t0.elapsed();
        if took > Duration::from_millis(100) && self.ui.slow_warned.is_none_or(|t| t.elapsed() > Duration::from_secs(60)) {
            self.ui.slow_warned = Some(Instant::now());
            log::warn!("slow frame: {} ms on the {} page", took.as_millis(), self.core.page.name());
        }
    }

    /// Logo and the page tabs.
    fn top_bar(&mut self, ui: &mut Ui) {
        let p = theme::pal(ui);
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::GOGGLES).size(24.0).color(p.accent));
            ui.label(RichText::new("omarchyvr").size(21.0).family(theme::heading(ui)).color(p.text));
            ui.add_space(28.0);
            let (_, warn, fail) = pimax_gui_core::checklist::counts(&self.core.items);
            let mut tabs = vec![
                (Page::Desk, icon::HOUSE, "Home".to_string()),
                (Page::Setup, icon::WRENCH, if fail + warn > 0 { format!("Ready ({})", fail + warn) } else { "Ready".to_string() }),
            ];
            tabs.push((Page::Ai, icon::ROBOT, "AI".to_string()));
            // Developer mode (Home, bottom): the advanced pages; a page already open stays reachable
            if self.core.state.dev_mode || self.core.page == Page::Tools {
                tabs.push((Page::Tools, icon::SQUARES_FOUR, "Tools".to_string()));
            }
            if self.core.state.dev_mode || self.core.page == Page::Logs {
                tabs.push((Page::Logs, icon::FILE_TEXT, "Logs".to_string()));
            }
            if self.core.state.dev_mode || self.core.page == Page::Profiles {
                tabs.push((Page::Profiles, icon::USERS, "Profiles".to_string()));
            }
            if self.core.state.dev_mode || self.core.page == Page::System {
                tabs.push((Page::System, icon::PULSE, "System".to_string()));
            }
            if !self.core.state.onboarded || self.core.page == Page::Onboarding {
                tabs.push((Page::Onboarding, icon::SPARKLE, "Onboarding".to_string()));
            }
            for (pg, ic, name) in tabs {
                let sel = self.core.page == pg;
                let id = egui::Id::new(("tab", pg.name()));
                let t = theme::ease(ui, id, sel, 0.18);
                let text = RichText::new(format!("{ic}  {name}")).size(14.0).color(theme::mix(p.dim, p.text, t));
                let b = egui::Button::new(text)
                    .fill(theme::mix(egui::Color32::TRANSPARENT, p.accent.gamma_multiply(0.22), t))
                    .stroke(egui::Stroke::new(1.0, theme::mix(egui::Color32::TRANSPARENT, p.accent.gamma_multiply(0.6), t)))
                    .corner_radius(egui::CornerRadius::same(255))
                    .min_size(egui::vec2(0.0, 32.0));
                let r = ui.add(b);
                r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
                if r.clicked() {
                    self.core.page = pg;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.core.demo {
                    widgets::chip(ui, "demo data", p.info);
                }
                if self.core.facts.at == 0 {
                    ui.label(RichText::new("checking this PC...").size(12.0).color(p.dim));
                    widgets::spinner(ui);
                } else {
                    let r = ui.add(egui::Button::new(RichText::new(format!("{} Re-check", icon::ARROWS_CLOCKWISE)).size(12.5).color(p.dim)).frame(false));
                    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Re-check"));
                    if r.on_hover_text("Run the doctor checks and read the kernel log again").clicked() {
                        self.core.refresh(true);
                        self.ui.pf = None;
                    }
                }
            });
        });
    }

    fn page_error_card(&mut self, ui: &mut Ui, page: Page, msg: &str) {
        widgets::card(ui, |ui| {
            ui.label(RichText::new("This page hit an internal error").size(20.0).strong().color(pal(ui).fail));
            widgets::note(ui, "The rest of the app works; nothing was stopped or started. The error is in the app's log and in a report under ~/.local/state/pimax/crashes.");
            widgets::code_box(ui, &[msg.to_string()]);
            ui.horizontal(|ui| {
                if ui.button("Try again").clicked() {
                    self.ui.page_error = None;
                }
                if ui.button("Report a problem...").clicked() {
                    self.ui.report_open = true;
                }
                if page != Page::Desk && ui.button("Go to Desk mode").clicked() {
                    self.core.page = Page::Desk;
                }
            });
        });
    }

    /// Device dots, the job or the last message, the output panel, settings and help.
    fn status_bar(&mut self, ui: &mut Ui) {
        let p = theme::pal(ui);
        let devs = pimax_gui_core::devices::devices(&self.core.facts, &self.core.live, &self.core.toggles, self.core.demo);
        if self.ui.show_log {
            let row_h = ui.text_style_height(&egui::TextStyle::Monospace).max(14.0);
            let n = self.core.log.len();
            widgets::inset(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("joblog")
                    .max_height(200.0)
                    .stick_to_bottom(true)
                    .auto_shrink([false, true])
                    .show_rows(ui, row_h, n, |ui, range| {
                        for l in &self.core.log[range] {
                            ui.add(egui::Label::new(RichText::new(l).monospace().size(12.0).color(p.dim)).truncate());
                        }
                    });
            });
            ui.add_space(4.0);
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for (id, d) in [("sb-h", &devs.headset), ("sb-l", &devs.left), ("sb-r", &devs.right)] {
                let c = theme::fade(ui, egui::Id::new(id), widgets::dev_color(&p, d.state));
                widgets::dot(ui, c, 3.5);
                ui.label(RichText::new(&d.name).size(11.5).color(p.dim)).on_hover_text(format!("{}: {}", d.label, d.detail));
                ui.add_space(8.0);
            }
            let c = theme::fade(ui, egui::Id::new("sb-t"), widgets::dev_color(&p, devs.tracking.1));
            ui.label(RichText::new(format!("{} {}", icon::CROSSHAIR, devs.tracking.0)).size(11.5).color(c));
            ui.add_space(14.0);
            if let Some(j) = &self.core.job {
                widgets::spinner(ui);
                ui.label(RichText::new(format!("{} ({:.0} s)", j.label, j.started.elapsed().as_secs_f64())).size(12.0).color(p.text));
            } else if !self.core.message.is_empty() {
                ui.add(egui::Label::new(RichText::new(self.core.message.clone()).size(12.0).color(p.dim)).truncate());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let h = ui.add(egui::Button::new(RichText::new(icon::QUESTION).size(16.0).color(p.dim)).frame(false));
                h.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Help"));
                if h.on_hover_text("Help").clicked() {
                    self.ui.help_open = true;
                }
                let g = ui.add(egui::Button::new(RichText::new(icon::GEAR).size(16.0).color(p.dim)).frame(false));
                g.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Setup checklist"));
                if g.on_hover_text("Setup checklist").clicked() {
                    self.core.page = Page::Setup;
                }
                let label: String = if self.ui.show_log { "Hide output".into() } else { format!("Output ({} lines)", self.core.log.len()) };
                if ui.add(egui::Button::new(RichText::new(label).size(11.5).color(p.dim)).frame(false)).clicked() {
                    self.ui.show_log = !self.ui.show_log;
                }
            });
        });
    }

    /// The round help button over the bottom right corner.
    fn help_button(&mut self, ui: &mut Ui) {
        let p = theme::pal(ui);
        let ctx = ui.ctx().clone();
        egui::Area::new(egui::Id::new("help-fab"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-26.0, -58.0))
            .order(egui::Order::Foreground)
            .show(&ctx, |ui| {
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(50.0, 50.0), egui::Sense::click());
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Help"));
                let hover = theme::ease(ui, egui::Id::new("help-fab-hover"), resp.hovered(), theme::T_HOVER);
                let fill = theme::mix(p.accent, p.accent_hover, hover);
                ui.painter().circle_filled(rect.center(), 25.0, fill);
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, icon::HEADSET, egui::FontId::proportional(22.0), egui::Color32::WHITE);
                if resp.on_hover_text("Help").clicked() {
                    self.ui.help_open = true;
                }
            });
    }

    /// Help: the controls guide, the docs, a problem report.
    fn help_dialog(&mut self, ui: &mut Ui) {
        if !self.ui.help_open || self.core.pending.is_some() || self.core.recovery.is_some() {
            return;
        }
        let mut close = false;
        let resp = egui::Modal::new(egui::Id::new("pimax-help")).show(ui.ctx(), |ui| {
            ui.set_max_width(560.0);
            ui.label(RichText::new("Help").size(20.0).family(theme::heading(ui)));
            widgets::note(ui, "Ready: checks the PC and the headset; Fix does what is needed. IPD: set your eye distance (Apply moves the lenses when VR is stopped). Room: map your room and floor.");
            widgets::note(ui, "Play starts VR for Steam and OpenXR games. Work starts the desk (screens over passthrough). Stop ends what runs.");
            widgets::note(ui, "Restart VR service restarts the running session with the same settings. Restart headset sends the headset its restart command (stop VR first; back in about 30 seconds).");
            widgets::note(ui, "Camera hands (Home > Tracking): in Work your bare hand is the pointer (pinch = click) and the controllers are ignored entirely unless you switch on \"Use controllers in Work\" (off by default). Say \"Max, use controllers\" or \"Max, hands only\" to switch: docs/HANDS.md.");
            widgets::note(ui, "Controller models (WayVR page): with controllers in use in Work you see a 3D Quest 2 style controller in each hand, like SteamVR shows (trigger, grip, stick and buttons move); the pointer ray leaves its front tip. \"Debug: show pointer and grip axes\" draws coloured axes to check that in the headset (next desk start): docs/WAYVR-CONTROLS.md.");
            widgets::note(ui, "Room setup (Home > Room): Fast setup (30 s: stand in the middle, look around, look forward and pull a trigger) or Full setup (walk the room, controller on the floor, trace the play area with a trigger). The steps show in the headset. With 'Put my room in place at every Play and Work' the floor, centre and forward come back every session; 'Tell SteamVR' gives SteamVR the same play area. docs/ROOM-SETUP.md.");
            widgets::note(ui, "View room map (Home > Room): opens your room map as a page in the browser, offline: turn it in 3D, switch to the floor plan, points coloured from floor to ceiling, the headset path as a line. Same page: pimax-roommap view. docs/GUI.md > Room.");
            widgets::note(ui, "Render quality (Home > Quality / Performance): Quality = the panel's own 3840x3552 per eye, Balanced = 85 %, Performance = 70 % (about half the pixels) for games that stutter or feel delayed. Applies at the next Play: docs/RENDER-QUALITY.md.");
            widgets::note(ui, "Brightness (Home > Quality / Performance): Image = Standard / Bright (default) / Brighter lifts the mid tones of games at the next start without burning out whites; Panel backlight (20 to 100 %) changes the headset right away. The backlight is already at its maximum on Linux: docs/RENDER-QUALITY.md.");
            widgets::note(ui, "Game mode (Home > Quality / Performance): Play pauses the local AI to free the graphics card and Stop starts it again; Sunshine keeps streaming unless you turn on \"Also pause Sunshine (stops game streaming)\". Compositor time budget (Auto / 6 ms / 8 ms): compare the late % and game fps on the session card: docs/RENDER-QUALITY.md.");
            widgets::note(ui, "Record (Home): Start recording saves what the headset shows as a video in ~/Videos/pimax. Record tracking saves one folder per take (cameras, controller logs, session log) for comparing with Windows: pick a scenario and a length, it works during Play, Work or SteamVR without disturbing them. docs/RECORD.md.");
            widgets::note(ui, "Controllers (Home): Pair - put the controller in pairing mode first, then press Pair for that hand only and wait up to 60 s (the other controller may drop out briefly meanwhile). Unpair disconnects one controller (left or right) from the headset; pair it again afterwards. Test vibration gives that controller a short buzz (also while VR runs); games and SteamVR vibrate the controllers by themselves: docs/STEAMVR.md section 6.");
            widgets::note(ui, "Sound and microphone (Home): Sound in headset sends all sound to the headset speakers while VR runs (Play, Work or SteamVR; they get it over the video cable a few seconds after the headset lights up) and Stop puts your old output back; Test headset sound plays two soft notes there. Headset microphone makes it the input for voice commands, games and calls while VR runs; Test microphone records 3 s, shows the level and plays it back (start VR first: the microphone is silent while the headset display is off). Sunshine is left alone.");
            widgets::note(ui, "Refresh rate (Home > Quality / Performance): 72 Hz or 90 Hz (new), applied when VR starts with the same commands Pimax Play sends on Windows.");
            widgets::note(ui, "Steam games: Ready > \"Steam set up for VR games\" > Fix sets Steam up once (then quit and start Steam once); no game needs launch options. Games run with (Home): Monado, or SteamVR (experimental: Play starts SteamVR with the Pimax driver, Stop Play closes it; Play starts Steam first the normal way so SteamVR never starts Steam with its own settings, and Home says so when Steam restarts or SteamVR quits during play): docs/STEAMVR.md.");
            ui.add_space(6.0);
            let docs = self.core.paths.repo.join("docs");
            ui.horizontal_wrapped(|ui| {
                if widgets::ibutton(ui, icon::BOOK_OPEN, "SteamVR", true).clicked() {
                    self.core.open_folder(&docs.join("STEAMVR.md"));
                }
                if widgets::ibutton(ui, icon::BOOK_OPEN, "Render quality", true).clicked() {
                    self.core.open_folder(&docs.join("RENDER-QUALITY.md"));
                }
                if widgets::ibutton(ui, icon::BOOK_OPEN, "WayVR controls", true).clicked() {
                    self.core.open_folder(&docs.join("WAYVR-CONTROLS.md"));
                }
                if widgets::ibutton(ui, icon::BOOK_OPEN, "App guide (GUI.md)", true).clicked() {
                    self.core.open_folder(&docs.join("GUI.md"));
                }
                // Developer mode only: the lessons are for developers
                if self.core.state.dev_mode && widgets::ibutton(ui, icon::LIST, "Lessons", true).clicked() {
                    self.core.page = Page::Logs;
                    self.ui.logs_tab = LogsTab::Lessons;
                    close = true;
                }
                if widgets::ibutton(ui, icon::BUG, "Report a problem...", true).clicked() {
                    self.ui.report_open = true;
                    close = true;
                }
                if widgets::ibutton(ui, icon::SPARKLE, "Onboarding", true).on_hover_text("The headset setup steps again").clicked() {
                    self.core.page = Page::Onboarding;
                    close = true;
                }
            });
            ui.add_space(8.0);
            if widgets::primary(ui, "Close", true).clicked() {
                close = true;
            }
        });
        if resp.should_close() || close {
            self.ui.help_open = false;
        }
    }

    fn modal(&mut self, ui: &mut Ui) {
        let Some(p) = self.core.pending.clone() else { return };
        let mut close = false;
        let mut confirm = false;
        let resp = egui::Modal::new(egui::Id::new("pimax-pending")).show(ui.ctx(), |ui| {
            ui.set_max_width(720.0);
            match &p {
                Pending::Root { title, fix, catalogue } => {
                    ui.label(RichText::new(format!("Root fix: {title}")).size(18.0).strong());
                    widgets::note(ui, &fix.why);
                    if let Some(c) = catalogue {
                        let what = if c.undo { "the undo of catalogue fix" } else { "catalogue fix" };
                        ui.label(RichText::new(format!("{what} {} (logged to ~/.local/state/pimax/fixes.log)", c.planned.fix.id)).size(11.5).color(pal(ui).muted));
                    }
                    ui.add_space(6.0);
                    ui.label("These lines need root (as documented):");
                    widgets::code_box(ui, &fix.lines);
                    match fix.command() {
                        Some(c) => {
                            ui.add_space(4.0);
                            ui.label(if self.core.facts.polkit_agent {
                                "The app will run exactly this; polkit asks for your password:"
                            } else {
                                "No polkit agent runs: this opens in a terminal, where pkexec asks for your password:"
                            });
                            widgets::code_box(ui, &[c.display()]);
                        }
                        None => {
                            ui.colored_label(pal(ui).warn, format!("Copy only: {}", fix.pkexec.clone().err().unwrap_or_default()));
                        }
                    }
                    if fix.reboot {
                        ui.colored_label(pal(ui).warn, "Reboot afterwards (the app never reboots for you).");
                    }
                    if !fix.undo.is_empty() {
                        egui::CollapsingHeader::new("Undo").id_salt("undo").show(ui, |ui| {
                            widgets::code_box(ui, &fix.undo);
                        });
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if fix.command().is_some() && widgets::primary(ui, "Run with pkexec", !self.core.busy()).clicked() {
                            confirm = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                }
                Pending::Confirm { title, detail, command, .. } => {
                    ui.label(RichText::new(title).size(18.0).strong());
                    widgets::note(ui, detail);
                    ui.add_space(6.0);
                    ui.label("Runs:");
                    widgets::code_box(ui, std::slice::from_ref(command));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if widgets::primary(ui, "Run", !self.core.busy()).clicked() {
                            confirm = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                }
                Pending::Interrupt { pid, comm, .. } => {
                    ui.label(RichText::new(format!("Send SIGINT to pid {pid} ({comm})?")).size(18.0).strong());
                    widgets::note(ui, "A polite stop by process id (never by name, never SIGKILL). The name is checked again right before; monado-service is never signalled here (it stops through pimax vr stop --grace 30).");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if widgets::primary(ui, "Send SIGINT", true).clicked() {
                            confirm = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                }
            }
        });
        // Esc or a click next to the dialog cancels it (nothing runs without the explicit button)
        if resp.should_close() && !confirm {
            close = true;
        }
        if confirm {
            match p {
                Pending::Root { .. } => self.core.confirm_root(),
                Pending::Confirm { .. } => self.core.confirm_step(),
                Pending::Interrupt { .. } => self.core.confirm_interrupt(),
            }
        } else if close {
            self.core.cancel_pending();
        }
    }
}

impl PimaxApp {
    /// After a crash: what happened, what still runs, and what to do with the report.
    fn recovery_dialog(&mut self, ui: &mut Ui) {
        let Some(rec) = self.core.recovery.clone() else { return };
        if self.core.pending.is_some() {
            return;
        }
        let orphans = self.core.orphans();
        let mut close = false;
        let mut act = None;
        let resp = egui::Modal::new(egui::Id::new("pimax-recovery")).show(ui.ctx(), |ui| {
            ui.set_max_width(760.0);
            let p = pal(ui);
            ui.label(RichText::new(rec.report.kind.title()).size(20.0).strong().color(p.warn));
            ui.label(RichText::new(format!("{}  ({})", rec.report.summary, rec.report.when)).color(p.muted));
            ui.add_space(6.0);
            widgets::note(ui, "The app started again. Nothing was stopped: a desk or VR session keeps running without the app.");
            if !orphans.is_empty() {
                ui.add_space(6.0);
                widgets::title(ui, "Still running");
                for o in &orphans {
                    ui.horizontal_wrapped(|ui| {
                        ui.add(egui::Label::new(o.text.as_str()).wrap());
                        if let Some(a) = &o.action {
                            let label = match a {
                                pimax_gui_core::preflight::Action::StopSession => "Stop session".to_string(),
                                pimax_gui_core::preflight::Action::StopForeignMonado => "Stop it".to_string(),
                                pimax_gui_core::preflight::Action::InterruptPid { pid, comm } => format!("SIGINT {comm} ({pid})"),
                                _ => "Open".to_string(),
                            };
                            if ui.add_enabled(!self.core.busy(), egui::Button::new(label)).clicked() {
                                act = Some(a.clone());
                            }
                        }
                    });
                }
            }
            ui.add_space(6.0);
            egui::CollapsingHeader::new("The report").id_salt("crash-report").show(ui, |ui| {
                egui::ScrollArea::vertical().id_salt("crash-report-text").max_height(260.0).show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(rec.report.render()).monospace().size(11.0)).wrap());
                });
            });
            ui.label(RichText::new(rec.dir.display().to_string()).small().color(p.muted));
            if rec.more > 0 {
                ui.label(RichText::new(format!("{} more report(s) follow.", rec.more)).small().color(p.muted));
            }
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button("Copy report").clicked() {
                    ui.ctx().copy_text(rec.report.render());
                    self.core.message = "Report copied".into();
                }
                if ui.button("Open folder").clicked() {
                    self.core.open_folder(&rec.dir);
                }
                if ui.button("Report a problem...").clicked() {
                    self.ui.report_open = true;
                    close = true;
                }
                if widgets::primary(ui, "Close", true).clicked() {
                    close = true;
                }
            });
        });
        if resp.should_close() {
            close = true;
        }
        if let Some(a) = act {
            self.core.act(&a);
        }
        if close {
            self.core.dismiss_recovery();
        }
    }

    /// "Report a problem": what goes in, the dump opt-in, the result.
    fn report_dialog(&mut self, ui: &mut Ui) {
        if !self.ui.report_open || self.core.recovery.is_some() || self.core.pending.is_some() {
            return;
        }
        let mut close = false;
        let resp = egui::Modal::new(egui::Id::new("pimax-report")).show(ui.ctx(), |ui| {
            ui.set_max_width(700.0);
            let p = pal(ui);
            ui.label(RichText::new("Report a problem").size(20.0).strong());
            widgets::note(ui, "Writes one .tar.gz with the app's log, the setup checks, the desk preflight, this boot's GPU kernel events, the end of the session logs, the last action outputs and the crash reports. Your home directory, user and host names, serial numbers, machine id, e-mail, IP and MAC addresses are replaced first. Nothing is sent anywhere: share the file yourself.");
            ui.add_space(4.0);
            ui.checkbox(&mut self.core.problem.include_minidumps, "Also include crash dumps (raw memory of the crashed window; not redacted)");
            ui.add_space(6.0);
            match &self.core.problem.result {
                _ if self.core.problem.busy => {
                    ui.horizontal(|ui| {
                        widgets::spinner(ui);
                        ui.label("Writing the report...");
                    });
                }
                Some(Ok(path)) => {
                    ui.colored_label(p.ok, "Report written:");
                    widgets::code_box(ui, &[path.display().to_string()]);
                    if ui.button("Open folder").clicked() {
                        let dir = path.parent().map(|d| d.to_path_buf()).unwrap_or_default();
                        self.core.open_folder(&dir);
                    }
                }
                Some(Err(e)) => {
                    ui.colored_label(p.fail, format!("The report failed: {e}"));
                }
                None => {}
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if widgets::primary(ui, "Create report", !self.core.problem.busy).clicked() {
                    self.core.report_problem();
                }
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        });
        if resp.should_close() && !self.core.problem.busy {
            close = true;
        }
        if close {
            self.ui.report_open = false;
        }
    }
}

impl eframe::App for PimaxApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.ui.min_frame.is_zero() && self.ui.frames == 0 {
            self.ui.min_frame = Duration::from_millis(16);
        }
        self.logic_step(ctx);
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.core.flush_state();
        log::info!("window closing after {} frames", self.ui.frames);
    }
}
