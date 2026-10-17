//! The window: the sidebar with the pages, the action row (games runtime, restarts, room setup), the page, the status
//! bar with the job log, and the dialogs. All state and every action live in [`omarchyvr_gui_core::app::AppCore`].
use crate::art::{self, Art};
use crate::pages;
use crate::theme::{self, caps, icon};
use crate::widgets::{self, pal};
use eframe::egui::{self, RichText, Ui};
use omarchyvr_gui_core::app::{AppCore, DeskPhase, Page, Pending};
use omarchyvr_gui_core::crash;
use omarchyvr_gui_core::devices::Dev;
use omarchyvr_gui_core::launcher::DeskToggles;
use omarchyvr_gui_core::preflight::Preflight;
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
    /// The help dialog is open.
    pub help_open: bool,
    /// The page shown and when it was opened (for the fade-in).
    pub page_t0: Option<(Page, f64)>,
    /// Setup page tab.
    pub setup_tab: SetupTab,
    /// The headset picked on the onboarding's first step (not used yet).
    pub onb_pick: Option<&'static str>,
    /// The AI provider being added or edited.
    pub ai_draft: Option<crate::pages::ai::AiDraft>,
    /// When the AI page was drawn last (egui time): a gap means it was opened again, so the Claude CLI is looked at again.
    pub ai_seen: f64,
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
            help_open: false,
            page_t0: None,
            setup_tab: SetupTab::Checks,
            onb_pick: None,
            ai_draft: None,
            ai_seen: f64::NEG_INFINITY,
        }
    }
}

/// The app.
pub struct PimaxApp {
    /// Core state.
    pub core: AppCore,
    /// UI state.
    pub ui: UiState,
    /// The colours choice applied to the window (Appearance setting).
    pub appearance: crate::appearance::Appearance,
}

/// The widest a page gets (points); wider windows centre it.
const PAGE_MAX_W: f32 = 1280.0;

/// One entry of the sidebar.
struct Nav {
    page: Page,
    icon: &'static str,
    label: &'static str,
}

const MAIN: [Nav; 8] = [
    Nav { page: Page::Desk, icon: icon::HOUSE, label: "Home" },
    Nav { page: Page::Display, icon: icon::SUN, label: "Display" },
    Nav { page: Page::Tracking, icon: icon::CROSSHAIR, label: "Tracking" },
    Nav { page: Page::Room, icon: icon::MAP_TRIFOLD, label: "Room" },
    Nav { page: Page::Desktop, icon: icon::DESKTOP, label: "Desktop" },
    Nav { page: Page::Backgrounds, icon: icon::CUBE_FOCUS, label: "Backgrounds" },
    Nav { page: Page::Record, icon: icon::VIDEO_CAMERA, label: "Recording" },
    Nav { page: Page::Ai, icon: icon::BRAIN, label: "AI" },
];

/// Developer mode: the advanced pages.
const DEV: [Nav; 5] = [
    Nav { page: Page::Developer, icon: icon::CODE, label: "Developer" },
    Nav { page: Page::Tools, icon: icon::WRENCH, label: "Tools" },
    Nav { page: Page::Logs, icon: icon::FILE_TEXT, label: "Logs" },
    Nav { page: Page::Profiles, icon: icon::USERS, label: "Profiles" },
    Nav { page: Page::System, icon: icon::PULSE, label: "System" },
];

/// The name of a page in the sidebar.
pub fn page_label(p: Page) -> &'static str {
    match p {
        Page::Setup => "Ready",
        Page::Onboarding => "Onboarding",
        Page::WorkSetup => "Set up Work",
        Page::RoomSetup => "Room setup",
        _ => MAIN.iter().chain(DEV.iter()).find(|n| n.page == p).map(|n| n.label).unwrap_or("Home"),
    }
}

/// The help page: one short paragraph per feature (the "?" next to each setting says the same where it is).
const HELP: [(&str, &str); 37] = [
    ("Setting up your PC", "The first time the app opens it asks Which headset do you have (USB detection only pre-selects; you press Continue), shows Get the headset ready (cables and power, with live ticks; Start setup works even if a tick is missing; if the headset is asleep it shows live steps with animations: press the power button, and as the last resort unplug its POWER cable for 10 s and plug it back in, DisplayPort stays in; a second start of the app brings the window back, or replaces a window process that has no window), then sets up this PC: it looks at the PC, fixes what needs no password, runs the system part with ONE password prompt (steps shown as a list, Show details has the exact text), and checks everything again. All green: Play. A restart needed: Restart now (the app continues by itself afterwards). Something failed: each item says why and has one Fix button; several root fixes become Fix all (one password). A closed password window or a wrong password: Try again. Run setup again is on the Ready screen. IPD, controllers, eye calibration and the room map are optional (Next, in the headset)."),
    ("Install and uninstall", "Install: ./install.sh in the omarchyvr clone builds and installs everything and opens this app; the app then sets up this PC by itself: it checks, fixes what needs no password, asks for your password once for the system part (the list of steps shows live) and checks again. A restart only when the NVIDIA module or the boot image changed; the app then continues after the restart. Later Plays never ask. Uninstall: Ready > Uninstall omarchyvr (Show what would be removed first); downloads and recordings are kept, settings go into one backup file in your home. docs/INSTALL.md, docs/UNINSTALL.md."),
    ("Your headset data", "The factory calibration, the controller models, the lens profile, the gyro bias, the IPD, the eye calibration and the room map belong to your headset: uninstalling keeps them, and installing brings back what an older install left on this PC (Checks: Your previous install, with Fix). Until your own is found or read from the headset, the shipped Pimax Dream Air reference stands in; Checks > Headset data and the Room setup tile say which. VR starts at once: the gyro starts with its saved bias and refines it when the headset rests. In Work a part that cannot start (controller tracking, the room view, voice) is left out and named. docs/INSTALL.md."),
    ("NVIDIA driver patch", "The Dream Air needs three small fixes in the NVIDIA open kernel module. They are applied to the tested driver versions (patches/nvidia/tested-versions.txt) and again after every driver update. On any other version nothing is patched unless you turn on Allow the patch on untested NVIDIA drivers (shown on the setup screen, then Run setup again, or Ready): the headset picture may break or the patched build may fail, but the desktop keeps working (the normal NVIDIA module is built as a fallback, and a restore snapshot is taken first). Changing the switch needs Apply (one password) so driver updates follow it. docs/NVIDIA-UNTESTED.md."),
    ("The \"?\" marks", "Point at a \"?\" or click it (in the headset, click): a small box says what the setting does. Click it again, click anywhere else or press Esc to close it."),
    ("Ready", "Ready checks the PC and the headset; Fix does what is needed. Fixes that need root show their lines and ask for your password first."),
    ("Play and Work", "Play starts VR for Steam and OpenXR games: 6DoF, the controllers and their tracking, and the whole graphics card for the game (the local AI, voice and the desk never run in Play; Home shows \"Graphics card: nothing of Work or the AI uses it\"). Work starts the desk (screens over passthrough, voice, the AI). Stop ends what runs, everything of it. While one runs, the other button switches: it stops the first completely, then starts the other. docs/PLAY-WORK-AUDIT.md."),
    ("Set up Work", "The first time you press Work, two questions come before VR starts (not if you already set the AI up: then they count as answered): the voice assistant (install Lara, and who answers free-form requests: Claude through the Claude Code on this PC or your own API key, a model on this PC, another compatible API, or none) and speech to text (the processor by default). Downloads show their progress and can be cancelled; files already here are reused. Lara goes first with every brain, Claude Code included: it answers the commands it knows, the rest goes to the brain. Then Work starts. Change the answers on the AI page (Work questions). docs/WORK-FIRST-RUN.md."),
    ("VR home", "Play opens the VR home in the headset (Home > VR home > Start in VR home, on by default): a small panel in WayVR's own look with Work, Games with SteamVR, your installed Steam VR games (each with the runtime it uses), Room setup Quick or Full, Recenter, Fix floor and Exit VR. The menu button of either controller (left menu, right Pi button) opens and closes it, also over a Monado game. Switching goes through the full stop and start, one at a time, with the progress in the headset. docs/VR-HOME.md."),
    ("Exit Work", "In the headset: the controller's menu button opens the Work menu; Exit Work there (or on the watch on your left wrist, the way with bare hands) asks once more, then stops everything of Work: the screens, voice, the AI, the desk's virtual displays, and puts the sound back. The same as Stop in this app. docs/WAYVR-CONTROLS.md."),
    ("Restarts", "Restart service (top bar) restarts the running session with the same settings. Restart headset sends the headset its restart command (stop VR first; back in about 30 seconds)."),
    ("Games run with", "Top bar: Monado (xrizer), or SteamVR (experimental: Play starts SteamVR with the Pimax driver, Stop closes it; Play starts Steam first the normal way; if a Steam that SteamVR once started is still open, Play asks \"Restart Steam now?\" and only closes it on your yes). Steam set up once: Ready > \"Steam set up for VR games\" > Fix, then quit and start Steam once; no game needs launch options. docs/STEAMVR.md."),
    ("Display", "Refresh rate 90 Hz (default) or 72 Hz, applied when VR starts with the commands Pimax Play sends on Windows. Render quality: Quality = the panel's own 3840x3552 per eye, Balanced = 85 %, Performance = 70 % (about half the pixels) for games that stutter. Applies at the next Play. docs/RENDER-QUALITY.md."),
    ("Colours", "Display page, Appearance: the app follows your Omarchy theme (default) and changes with it within a couple of seconds, or uses its own omarchyvr colours, or one installed Omarchy theme you pick from the list. Light and dark themes both work; text that would be hard to read in a theme keeps a readable colour."),
    ("Brightness", "Image (Display page): Standard / Bright (default) / Brighter lifts the mid tones of games at the next start without burning out whites. Panel backlight (20 to 100 %) changes the headset right away; it is already at its maximum on Linux."),
    ("Game mode", "Play always frees the graphics card: the local AI and voice are stopped before a game session starts and come back only with Work; Sunshine is never paused or stopped. Light game mode (on by default) also leaves out the wrist readout and the room search; Eye tracking in games is its own switch (off by default). Compositor budget (Auto / 6 ms / 8 ms): compare the late % and game fps of the session line on Home. docs/GAME-MODE.md."),
    ("Web pages in VR", "Desktop page, Web pages in VR: during Work type an address and press Open in VR; the page opens in your browser on the VR desk (a flat screen you use with the controller pointer). A page cannot take over the headset (WebXR): Chrome, Chromium and Firefox on Linux have no OpenXR yet, and a page may only enter VR after a click anyway. WebGPU pages work in Chrome without any flag (the page shows whether it ran). Nothing runs in the background for this. docs/research/webxr.md."),
    ("IPD", "Display page: set your eye distance (58-72 mm). Apply moves the lenses when VR is stopped and saves it for the next start."),
    ("Tracking", "6DoF follows your head with the headset cameras: start it in a lit room with the cameras free. Eye tracking is visual only by default: a dot where you look in Work; your eyes never click, point or focus unless you turn on \"Eyes as a mouse\" (Tracking > Eyes, off by default; docs/EYE.md). It starts with every session once calibrated. In a dim room the cameras raise their exposure by themselves; if even that is too dark, the headset and the Tracking page say \"Tracking cameras see too little infrared light\" (daylight or an IR lamp helps). docs/AUTO-EXPOSURE.md."),
    ("Camera hands", "Tracking page (developer mode): in Work your bare hand is the pointer (pinch = click) and the controllers are ignored unless \"Controllers in Work\" is on (off by default). Say \"Max, use controllers\" or \"Max, hands only\" to switch. The two hand models have no licence, so they are not shipped: press \"Download hand models\" there, read the notice and agree; the download runs as you (needs git-lfs) and the switch turns on when it ends. docs/HANDS.md."),
    ("Controllers", "Home: Pair - put the controller in pairing mode first, then press Pair for that hand only and wait up to 60 s (the other controller may drop out briefly). Unpair disconnects one controller; pair it again afterwards. Vibrate gives it a short buzz (also while VR runs). docs/STEAMVR.md section 6."),
    ("Controllers when picked up", "Home, Controllers card: In Work (default When picked up) a controller lying on the desk is hidden, so it never shows or jumps around; pick it up or touch a button and it is back at once, 5 s still puts it away. In games it is Always by default. Each controller shows \"in hand\" or \"put down\" while VR runs. Voice: \"Max, controllers on / off\". docs/CONTROLLERS-INHAND.md."),
    ("Controller angle", "Tracking page, Controllers: the slider (-20 to +70 degrees, default 30) tilts the controller pointer and the hand on it up or down. Raise it if the ray points lower than you aim. It changes a running session within a second and is kept for the next start. docs/CONTROLLERS-INHAND.md."),
    ("Tracking stability", "Tracking page: New camera exposure (a bright window no longer makes the room pulse), Smooth room corrections (the room glides into place instead of jumping) and Steadier controllers (less shimmer at the edge of the view). All on by default; switch one off only if tracking got worse with it, from the next start. Each shows whether it is on in the running session. docs/TRACKING-STABILITY.md."),
    ("Pointing moves the cursor", "Desktop page, Next desk start. Off by default: pointing at a screen with a controller or hand leaves your real desktop cursor where it is; it moves when you click, press the trigger or grab. docs/WAYVR-CONTROLS.md."),
    ("Auto-arrange apps", "Desktop page, Next desk start. Off by default: screens and apps move only when you ask (a voice command like \"move left\", or a mode with --replace); nothing is re-sorted while you work. On: apps of different kinds are sent to their own screens. docs/SCREENS.md."),
    ("Controller models", "Desktop page (developer mode): with controllers in use in Work you see a 3D controller in each hand, like SteamVR shows; the pointer ray leaves its front tip. \"Pointer and grip axes\" draws coloured axes to check that in the headset. docs/WAYVR-CONTROLS.md."),
    ("Sound and microphone", "Home: Headset sound sends all sound to the headset speakers while VR runs (they get it over the video cable a few seconds after the headset lights up); Stop puts your old output back. Test plays two soft notes there. Headset mic makes it the input for voice, games and calls while VR runs; Test records 3 s and plays it back (start VR first: the microphone is silent while the display is off). Sunshine is left alone."),
    ("Room setup", "The third big button on Home and the Ready screen, under Play and Work: Calibrate and map your room. One guided flow: it reads your headset's factory calibration if this PC does not have it (a sleeping headset gets the wake steps), then the room setup (Fast 30 s or Full) guided in the headset, then a summary with ticks. Work and Play ask for it when the calibration is missing. docs/ROOM-SETUP.md."),
    ("Room", "Room page: Fast setup (30 s: stand in the middle, look around, look forward and pull a trigger) or Full setup (walk the room, controller on the floor, trace the play area with a trigger). The steps show in the headset. \"Room at every start\" puts the floor, centre and forward back every session; \"Share with SteamVR\" gives SteamVR the same play area. View room map opens the map in the browser, offline. docs/ROOM-SETUP.md."),
    ("Desktop", "Passthrough shows the room behind the screens. Presets: Cinema (curved screens, dimmed room), Work (flat, sharper screens), Assistant (voice captions on). Screen numbers: 1 main, 2 left, 3 right, 4 up, 5 up-left, 6 up-right."),
    ("Screen layout", "Up to 6 VR screens in two rows of three around you (up-left, up, up-right over left, main, right), placed so that none hides another from where you sit. Reset screens (double-click B, or Desktop > WayVR now) puts them back. \"Max, move this left\" puts the screen to the left of its neighbour, \"put screen 2 above screen 1\", \"bring this screen here\": each is placed once from where your head is now, and the other screens move aside just enough. docs/SCREENS.md."),
    ("Backgrounds", "Backgrounds page: the VR environment around you (not the desktop wallpaper). Tap a picture to show it, pick a resolution (Auto, 8K, 4K, 2K), Off / passthrough brings the room back, Download more fetches the free pictures. \"Ocean (animated)\" is a live sea and sky drawn behind your screens in Work (Quality low, medium, high; never in Play). Work starts with your choice. Say \"Max, set background flowers\" (universe, ocean, Hawaii, sunset ...), \"Max, next background\" or \"Max, background off\". docs/BACKGROUNDS.md."),
    ("Voice", "In Work: pause as you like and \"Max\" may come mid-sentence. Screens: \"Max, four screens\", \"a screen up there\", \"open YouTube in a new window\", \"all the terminals on one screen\". Two things at once: \"... and ...\". Apps to screens: \"Max, move Chrome to screen 3\", \"Max, swap screens 1 and 2\", \"Max, save this layout\". Hide screens (apps stay open): \"Max, close screen 2\", \"remove screens 3 and 4\", \"close everything\". Sizes: \"make all screens half the size\". \"Max, stop the VR\" stops Work like Stop and always asks \"yes\" first. docs/VOICE.md."),
    ("AI", "One AI page. Max's brain picks who answers voice commands the offline matcher does not know: Claude (CLI), the default, which uses the Claude Code you are logged into (no key; each command starts a fresh Claude with only your words and the screen state; Haiku fast or Sonnet; Test shows the time; Start Claude ahead keeps one ready so answers come sooner), a Local model on this PC (Work only: Low / Medium / High, Advanced to choose the router Lara, the action model and the video memory; speech runs on the processor, Speech on graphics card moves it to the card; one Test checks Lara, the action model, speech and the agent model), or Online with a key (Online providers). If Claude Code is not installed or not logged in, the Claude (CLI) card says so in plain words. Autofix cannot use the Claude CLI. docs/AI.md."),
    ("Agent mode", "AI page, Local model section: \"Max, let's enter cinema mode, I want to watch Dune\" or \"Max, set up work: terminals left, browser middle, YouTube up-right\" runs step by step (each step shows in the headset); \"Max, cancel\" stops it. docs/VOICE.md."),
    ("Recording", "Start recording saves what the headset shows as a video in ~/Videos/pimax. Record tracking saves one folder per take (cameras, controller logs, session log) for comparing with Windows; it never disturbs a running session. docs/RECORD.md."),
];

impl PimaxApp {
    /// Wrap a core.
    pub fn new(core: AppCore) -> Self {
        let appearance = crate::appearance::Appearance::new(!core.demo);
        PimaxApp { core, ui: UiState::default(), appearance }
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
        if ctx.data(|d| d.get_temp::<u64>(egui::Id::new("omarchyvr-theme-installed"))).is_none() {
            // a context without the theme (tests): install it; it is complete from the next frame on
            theme::install(ctx, true);
            ctx.request_repaint();
        }
        self.appearance.tick(ctx, self.core.state.colours);
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
            crate::window::write_ack();
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
        if std::mem::take(&mut self.core.room_requested) {
            // the onboarding's room step
            self.core.page = Page::Room;
        }
        let before = self.core.toggles;
        let side_w = if ui.available_width() < 1000.0 { 172.0 } else { 192.0 };
        egui::Panel::bottom("status")
            .frame(egui::Frame::new().fill(p.bar).inner_margin(egui::Margin::symmetric(14, 5)).stroke(egui::Stroke::new(1.0, p.stroke)))
            .show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("nav")
            .exact_size(side_w)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(p.bar).inner_margin(egui::Margin { left: 10, right: 10, top: 14, bottom: 10 }).stroke(egui::Stroke::new(1.0, p.stroke)))
            .show(ui, |ui| self.sidebar(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.bg).inner_margin(egui::Margin { left: 18, right: 18, top: 10, bottom: 0 }))
            .show(ui, |ui| {
                let page = self.core.page;
                if page != Page::System {
                    // the System page's event stream stops a few seconds after it was left
                    self.core.system_closed();
                }
                // the action row lines up with the page (both keep a readable width, centred, on very wide windows)
                let w = ui.available_width();
                let pad = ((w - PAGE_MAX_W) / 2.0).max(0.0).floor();
                ui.horizontal_top(|ui| {
                    ui.add_space(pad);
                    ui.vertical(|ui| {
                        ui.set_width(w.min(PAGE_MAX_W) - 2.0);
                        self.action_row(ui);
                    });
                });
                ui.add_space(10.0);
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
                egui::ScrollArea::vertical().id_salt(("page", page.name())).auto_shrink([false, false]).show(ui, |ui| {
                    // very wide windows: the page keeps a readable width, centred
                    let w = ui.available_width();
                    let pad = ((w - PAGE_MAX_W) / 2.0).max(0.0).floor();
                    ui.horizontal_top(|ui| {
                        ui.add_space(pad);
                        ui.vertical(|ui| {
                            ui.set_width(w.min(PAGE_MAX_W) - 2.0);
                            ui.spacing_mut().item_spacing.y = 12.0;
                            match self.ui.page_error.clone() {
                                Some((p, msg)) if p == page => self.page_error_card(ui, p, &msg),
                                _ => {
                                    let r = crash::catching(|| self.page(ui, page));
                                    if let Err(e) = r {
                                        let msg = omarchyvr_gui_core::worker::panic_text(e.as_ref());
                                        log::error!("the {} page failed: {msg}", page.name());
                                        self.ui.page_error = Some((page, msg));
                                    }
                                }
                            }
                            ui.add_space(14.0);
                        });
                    });
                });
            });
        if self.core.toggles != before {
            // a switch of the next start changed (any page): judge again and keep it
            self.ui.pf = None;
            self.core.save_state();
        }
        self.modal(ui);
        self.recovery_dialog(ui);
        self.report_dialog(ui);
        self.help_dialog(ui);
        if !had_job && self.core.job.is_some() {
            // an action started: show its progress
            self.ui.show_log = true;
        }
        if !crate::window::FIRST_FRAME.swap(true, Ordering::Relaxed) {
            crate::window::write_ack();
        }
        let took = t0.elapsed();
        if took > Duration::from_millis(100) && self.ui.slow_warned.is_none_or(|t| t.elapsed() > Duration::from_secs(60)) {
            self.ui.slow_warned = Some(Instant::now());
            log::warn!("slow frame: {} ms on the {} page", took.as_millis(), self.core.page.name());
        }
    }

    /// The page itself.
    fn page(&mut self, ui: &mut Ui, page: Page) {
        match page {
            Page::Desk => pages::home::show(ui, self),
            Page::Display => pages::display::show(ui, self),
            Page::Tracking => pages::tracking::show(ui, self),
            Page::Room => pages::room::show(ui, self),
            Page::Desktop => pages::desktop::show(ui, self),
            Page::Developer => pages::developer::show(ui, self),
            Page::Setup => pages::setup::show(ui, self),
            Page::Tools => pages::tools::show(ui, self),
            Page::Logs => pages::logs::show(ui, self),
            Page::Profiles => pages::profiles::show(ui, self),
            Page::Ai => pages::ai::show(ui, self),
            Page::Record => pages::record::show(ui, self),
            Page::Backgrounds => pages::backgrounds::show(ui, self),
            Page::System => pages::system::show(ui, self),
            Page::Onboarding => pages::onboarding::show(ui, self),
            Page::WorkSetup => pages::worksetup::show(ui, self),
            Page::RoomSetup => pages::roomsetup::show(ui, self),
        }
    }

    /// The mark, the pages, the developer switch.
    fn sidebar(&mut self, ui: &mut Ui) {
        let p = theme::pal(ui);
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            art::show(ui, Art::Mark, egui::vec2(30.0, 19.0), p.accent, p.dim);
            ui.label(caps("omarchyvr", 15.0, p.text).family(theme::heading(ui)));
        });
        ui.add_space(14.0);
        let dev = self.core.state.dev_mode;
        let (_, warn, fail) = omarchyvr_gui_core::checklist::counts(&self.core.items);
        let current = self.core.page;
        let mut go = None;
        // the developer switch and help stay at the bottom; the list scrolls when the window is short
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            let mut dv = dev;
            let r = ui
                .horizontal(|ui| {
                    ui.add_space(4.0);
                    let r = widgets::switch(ui, &mut dv, "Developer mode", true);
                    ui.label(caps("Developer", 10.5, p.dim));
                    r
                })
                .inner
                .on_hover_text("Shows the advanced settings and pages: tests, screens, voice, auto mode, logs, tools.");
            if r.changed() {
                self.core.set_dev_mode(dv);
            }
            ui.add_space(6.0);
            if (!self.core.state.onboarded || current == Page::Onboarding) && widgets::nav_item(ui, current == Page::Onboarding, icon::SPARKLE, "Onboarding", None).clicked() {
                go = Some(Page::Onboarding);
            }
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                egui::ScrollArea::vertical().id_salt("nav").auto_shrink([false, false]).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for n in &MAIN {
                        if widgets::nav_item(ui, current == n.page, n.icon, n.label, None).clicked() {
                            go = Some(n.page);
                        }
                    }
                    nav_separator(ui, &p);
                    let badge = (fail + warn > 0).then(|| ((fail + warn).to_string(), if fail > 0 { p.fail } else { p.warn }));
                    if widgets::nav_item(ui, current == Page::Setup, icon::SEAL_CHECK, "Ready", badge).clicked() {
                        go = Some(Page::Setup);
                    }
                    // Developer mode: the advanced pages; a page already open stays reachable
                    let devs: Vec<&Nav> = DEV.iter().filter(|n| dev || current == n.page).collect();
                    if !devs.is_empty() {
                        nav_separator(ui, &p);
                        for n in devs {
                            if widgets::nav_item(ui, current == n.page, n.icon, n.label, None).clicked() {
                                go = Some(n.page);
                            }
                        }
                    }
                });
            });
        });
        if let Some(pg) = go {
            self.core.page = pg;
        }
    }

    /// The row over every page: what games run with on the left; restart service, restart headset, room setup and
    /// re-check on the right.
    fn action_row(&mut self, ui: &mut Ui) {
        let p = theme::pal(ui);
        let compact = ui.available_width() < 800.0;
        ui.horizontal(|ui| {
            ui.set_min_height(28.0);
            self.games_runtime(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if self.core.demo {
                    widgets::chip(ui, "demo data", p.info);
                }
                if self.core.facts.at == 0 {
                    widgets::spinner(ui).on_hover_text("Checking this PC");
                } else {
                    let r = ui.add(egui::Button::new(RichText::new(icon::ARROWS_CLOCKWISE).size(14.0).color(p.dim)).frame(false));
                    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Re-check"));
                    if r.on_hover_text("Re-check: the doctor checks and the kernel log again").clicked() {
                        self.core.refresh(true);
                        self.ui.pf = None;
                    }
                }
                if widgets::ghost_btn(ui, icon::MAP_TRIFOLD, "Room setup", true, compact).clicked() {
                    self.core.page = Page::Room;
                }
                let hd = self.core.restart_headset_blocker();
                let r = widgets::ghost_btn(ui, icon::ARROW_COUNTER_CLOCKWISE, "Restart headset", hd.is_none(), compact);
                let r = match &hd {
                    Some(b) => r.on_disabled_hover_text(b.as_str()),
                    None => r.on_hover_text("The headset's own restart command (what Pimax Play's restart sends): back on USB after about 8 s, a picture after about 22 s. Not while VR runs; keep it still."),
                };
                if r.clicked() {
                    self.core.restart_headset();
                }
                let svc = self.core.restart_service_blocker();
                let r = widgets::ghost_btn(ui, icon::POWER, "Restart service", svc.is_none(), compact);
                let r = match &svc {
                    Some(b) => r.on_disabled_hover_text(b.as_str()),
                    None => r.on_hover_text("Stops the running session (30 s grace) and starts it again with the same settings."),
                };
                if r.clicked() {
                    self.core.restart_service();
                }
            });
        });
    }

    /// What games run with (only the options this PC has); Play starts Monado or SteamVR from it
    /// (`~/.config/omarchyvr/games.toml`).
    fn games_runtime(&mut self, ui: &mut Ui) {
        let p = theme::pal(ui);
        let (installed, chosen) = (self.core.facts.steamvr_installed, self.core.facts.games_steamvr);
        let names: Vec<&str> = if installed || chosen { vec!["Monado (xrizer)", "SteamVR (experimental)"] } else { vec!["Monado (xrizer)"] };
        let sel = usize::from(chosen).min(names.len() - 1);
        let free = !self.core.busy() && self.core.phase() == DeskPhase::Idle;
        ui.label(caps("Games", 10.5, p.faint));
        let mut pick = None;
        let r = ui.add_enabled_ui(free, |ui| {
            egui::ComboBox::from_id_salt("games-runtime").selected_text(RichText::new(names[sel]).size(12.5)).width(170.0).show_ui(ui, |ui| {
                for (i, n) in names.iter().enumerate() {
                    if ui.selectable_label(i == sel, *n).clicked() && i != sel {
                        pick = Some(i);
                    }
                }
            })
        });
        let tip = if chosen { "Play starts SteamVR with the Pimax driver (no Monado); Stop closes it. Experimental: Help > Games run with." } else { "Play starts Monado; Steam games reach it through xrizer." };
        let r = r.response.on_hover_text(tip);
        if !free {
            r.on_disabled_hover_text("Stop VR first");
        }
        if let Some(i) = pick {
            self.core.runtime(if i == 0 { "xrizer" } else { "steamvr" });
        }
    }

    fn page_error_card(&mut self, ui: &mut Ui, page: Page, msg: &str) {
        widgets::section(ui, "This page hit an internal error", None, |ui| {
            widgets::status(ui, "The rest of the app works; nothing was stopped or started. The error is in the app's log.", pal(ui).fail);
            widgets::code_box(ui, &[msg.to_string()]);
            ui.horizontal(|ui| {
                if ui.button("Try again").clicked() {
                    self.ui.page_error = None;
                }
                if ui.button("Report a problem...").clicked() {
                    self.ui.report_open = true;
                }
                if page != Page::Desk && ui.button("Go to Home").clicked() {
                    self.core.page = Page::Desk;
                }
            });
        });
    }

    /// The headset, the controllers and the tracking in one line; the job or the last message; output, Ready and
    /// Help on the right.
    fn status_bar(&mut self, ui: &mut Ui) {
        let p = theme::pal(ui);
        let devs = omarchyvr_gui_core::devices::devices(&self.core.facts, &self.core.live, &self.core.toggles, self.core.demo);
        if self.ui.show_log {
            let row_h = ui.text_style_height(&egui::TextStyle::Monospace).max(14.0);
            let n = self.core.log.len();
            ui.add_space(4.0);
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
            let sep = |ui: &mut Ui| {
                ui.add_space(4.0);
                ui.label(RichText::new("|").size(11.0).color(p.stroke_strong));
                ui.add_space(4.0);
            };
            let hc = theme::fade(ui, egui::Id::new("sb-h"), widgets::dev_color(&p, devs.headset.state));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                widgets::dot(ui, hc, 3.0);
                ui.label(caps(&devs.headset.name, 10.0, p.dim));
            })
            .response
            .on_hover_text(format!("{}: {}", devs.headset.label, devs.headset.detail));
            sep(ui);
            let up = pages::home::controllers_up(self);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                for (id, d) in [("sb-l", &devs.left), ("sb-r", &devs.right)] {
                    let c = theme::fade(ui, egui::Id::new(id), widgets::dev_color(&p, d.state));
                    widgets::dot(ui, c, 3.0);
                }
                ui.add_space(2.0);
                ui.label(caps(&format!("Controllers {up}/2"), 10.0, p.dim));
            })
            .response
            .on_hover_text(format!("{}: {}\n{}: {}", devs.left.name, devs.left.label, devs.right.name, devs.right.label));
            sep(ui);
            let tc = theme::fade(ui, egui::Id::new("sb-t"), widgets::dev_color(&p, devs.tracking.1));
            ui.label(RichText::new(icon::CROSSHAIR).size(11.0).color(tc));
            ui.label(caps(&format!("Tracking {}", devs.tracking.0), 10.0, if devs.tracking.1 == Dev::Off { p.dim } else { tc }));
            ui.add_space(12.0);
            // the links first, from the right: the job or the message is cut to what is left between
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                let link = |ui: &mut Ui, ic: &str, text: &str, name: &str| {
                    let r = ui.add(egui::Button::new(caps(&format!("{ic} {text}"), 10.0, p.dim)).frame(false));
                    let name = name.to_string();
                    r.widget_info(move || egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
                    r
                };
                if link(ui, icon::QUESTION, "Help", "Help").clicked() {
                    self.ui.help_open = true;
                }
                if link(ui, icon::SEAL_CHECK, "Ready", "Setup checklist").on_hover_text("The setup checks").clicked() {
                    self.core.page = Page::Setup;
                }
                let (text, name) = if self.ui.show_log { ("Hide output".to_string(), "Hide output".to_string()) } else { (format!("Output {}", self.core.log.len()), format!("Output ({} lines)", self.core.log.len())) };
                if link(ui, icon::TERMINAL, &text, &name).clicked() {
                    self.ui.show_log = !self.ui.show_log;
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if let Some(j) = &self.core.job {
                        widgets::spinner(ui);
                        ui.add(egui::Label::new(RichText::new(format!("{} ({:.0} s)", j.label, j.started.elapsed().as_secs_f64())).size(11.5).color(p.text)).truncate());
                    } else if !self.core.message.is_empty() {
                        ui.add(egui::Label::new(RichText::new(self.core.message.clone()).size(11.5).color(p.dim)).truncate());
                    }
                });
            });
        });
    }

    /// Help: one short paragraph per feature, the docs, a problem report.
    fn help_dialog(&mut self, ui: &mut Ui) {
        if !self.ui.help_open || self.core.pending.is_some() || self.core.recovery.is_some() {
            return;
        }
        let mut close = false;
        let max_h = (ui.ctx().content_rect().height() - 220.0).max(200.0);
        let resp = egui::Modal::new(egui::Id::new("pimax-help")).show(ui.ctx(), |ui| {
            ui.set_max_width(600.0);
            let p = theme::pal(ui);
            widgets::title(ui, "Help");
            ui.add_space(6.0);
            egui::ScrollArea::vertical().id_salt("help-text").max_height(max_h).show(ui, |ui| {
                for (head, text) in HELP {
                    ui.label(caps(head, 10.5, p.faint).strong());
                    widgets::note(ui, text);
                    ui.add_space(6.0);
                }
            });
            ui.add_space(8.0);
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
                if widgets::ibutton(ui, icon::SPARKLE, "Set up this PC", true).on_hover_text("Open the setup screen (run the setup again, next steps in the headset)").clicked() {
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
                        ui.label(RichText::new(format!("{what} {} (logged to ~/.local/state/omarchyvr/fixes.log)", c.planned.fix.id)).size(11.5).color(pal(ui).muted));
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
                Pending::RestartSteam { steam, .. } => {
                    let (title, body, busy) = AppCore::restart_steam_text(steam);
                    ui.set_max_width(460.0);
                    ui.label(RichText::new(title).size(18.0).strong());
                    ui.add_space(4.0);
                    widgets::note(ui, &body);
                    if let Some(b) = busy {
                        ui.add_space(4.0);
                        ui.colored_label(pal(ui).warn, b);
                    }
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if widgets::primary(ui, "Restart Steam", !self.core.busy()).clicked() {
                            confirm = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                }
                Pending::Interrupt { pid, comm, .. } => {
                    ui.label(RichText::new(format!("Send SIGINT to pid {pid} ({comm})?")).size(18.0).strong());
                    widgets::note(ui, "A polite stop by process id (never by name, never SIGKILL). The name is checked again right before; monado-service is never signalled here (it stops through omarchyvr vr stop --grace 30).");
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
                Pending::RestartSteam { .. } => self.core.confirm_steam_restart(),
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
            ui.label(RichText::new(rec.report.kind.title()).size(20.0).strong().color(p.warn_text));
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
                                omarchyvr_gui_core::preflight::Action::StopSession => "Stop session".to_string(),
                                omarchyvr_gui_core::preflight::Action::StopForeignMonado => "Stop it".to_string(),
                                omarchyvr_gui_core::preflight::Action::InterruptPid { pid, comm } => format!("SIGINT {comm} ({pid})"),
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
        let resp = egui::Modal::new(egui::Id::new("omarchyvr-report")).show(ui.ctx(), |ui| {
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

/// A thin line between groups of the sidebar.
fn nav_separator(ui: &mut Ui, p: &theme::Palette) {
    ui.add_space(5.0);
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(egui::vec2(w, 1.0), egui::Sense::hover());
    ui.painter().hline(r.x_range().shrink(6.0), r.center().y, egui::Stroke::new(1.0, p.stroke));
    ui.add_space(5.0);
}
