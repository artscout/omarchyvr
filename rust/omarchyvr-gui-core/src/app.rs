//! [`AppCore`]: the state of the whole app and every action a click can trigger. The UI crate only draws this and
//! calls these methods.
//!
//! Ordering guarantees: one job at a time; a desk start is `WaitNoMonado` (LESSONS row 1) -> a re-check of camera and
//! eye chip (rows 2, 3) -> `omarchyvr desk start ...` (row 5); `--force` only with the user's consent and only when the
//! preflight says the WayVR Xid 31 faults are the only problem (row 9). Stops go through `omarchyvr desk stop --grace 30`
//! / `omarchyvr vr stop --grace 30`; the only signal the app sends itself is SIGINT to one PID after a confirmation, never
//! to monado-service. Root fixes run only after the user confirmed the shown pkexec command.
use crate::builds::MarkerCache;
use crate::checklist::{self, Fix, FollowUp, Item, RootFix, UserFix};
use crate::cmd::Cmd;
use crate::docs;
use crate::facts::{collect_facts, collect_live, Facts, Live};
use crate::guards;
use crate::guistate::GuiState;
use crate::job::{self, Job, JobEvent, JobKind, JobSpec, Step};
use crate::launcher::{self, DeskPlan, DeskToggles};
use crate::machine::Machine;
use crate::monitor::{Monitor, Update};
use crate::monlog::LogFollower;
use crate::paths::Paths;
use crate::preflight::{self, Action, Gate, Preflight};
use crate::procs;
use crate::profiles::Profiles;
use crate::term;
use crate::tools::{self, Availability, Launch, ToolId, ToolOptions};
use crate::room::{self, RoomPaths, RoomState, Writer};
use crate::wayvr::{self, Ctl, Lint, Overlay, WayvrPaths, WayvrSettings};
use crate::worker::{Apply, Worker};
use crate::autofix::AutofixView;
use crate::findings::{self, CatalogueRun, FixAction};
use crate::onboarding::OnboardingView;
use crate::{bundle, crash, fixlog, orphans};
use omarchyvr_config::Store;
use omarchyvr_doctor::Opts;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Page {
    /// Home: the headset, the controllers, Play / Work, quick settings.
    Desk,
    /// Display: refresh rate, render quality, brightness, performance, IPD.
    Display,
    /// Tracking: 6DoF, controllers, hands, eye tracking.
    Tracking,
    /// Room setup, the room map and the floor.
    Room,
    /// The Work desk (WayVR): passthrough, presets, screens, subtitles, voice.
    Desktop,
    /// Developer: auto mode, the VR runtime, crash reports, what Start runs, the session details.
    Developer,
    /// Setup checklist.
    Setup,
    /// Tools.
    Tools,
    /// Logs and history.
    Logs,
    /// Profiles and backups.
    Profiles,
    /// AI: which AI answers voice commands (Claude CLI, a local model, online), their settings and providers.
    Ai,
    /// Recording: headset video and tracking takes.
    Record,
    /// VR backgrounds (the skybox library): pages/backgrounds.rs.
    Backgrounds,
    /// omarchyvrd: components, events, safe mode (the System page).
    System,
    /// Onboarding (the headset module's steps).
    Onboarding,
}

impl Page {
    /// Every page, in navigation order.
    pub const ALL: [Page; 15] = [
        Page::Desk,
        Page::Display,
        Page::Tracking,
        Page::Room,
        Page::Desktop,
        Page::Backgrounds,
        Page::Record,
        Page::Ai,
        Page::Setup,
        Page::Developer,
        Page::Tools,
        Page::Logs,
        Page::Profiles,
        Page::System,
        Page::Onboarding,
    ];

    /// Stable name (app state file, crash reports).
    pub fn name(self) -> &'static str {
        match self {
            Page::Desk => "desk",
            Page::Display => "display",
            Page::Tracking => "tracking",
            Page::Room => "room",
            Page::Desktop => "desktop",
            Page::Developer => "developer",
            Page::Setup => "setup",
            Page::Tools => "tools",
            Page::Logs => "logs",
            Page::Profiles => "profiles",
            Page::Ai => "ai",
            Page::Record => "record",
            Page::Backgrounds => "backgrounds",
            Page::System => "system",
            Page::Onboarding => "onboarding",
        }
    }

    /// From [`Page::name`].
    pub fn from_name(s: &str) -> Option<Page> {
        // "wizard": state files of the app before the onboarding replaced the first-run wizard
        let s = match s {
            "wizard" => "onboarding",
            // the Local AI page became a section of the AI page
            "local-ai" => "ai",
            s => s,
        };
        Page::ALL.into_iter().find(|p| p.name() == s)
    }
}

/// What the Desk page shows as the big state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeskPhase {
    /// Nothing runs.
    Idle,
    /// The start job runs.
    Starting,
    /// Our session runs.
    Running,
    /// The stop job runs, or Monado is still exiting.
    Stopping,
    /// Another job runs (a fix, a tool).
    Busy,
    /// A monado-service runs that the launcher did not start.
    Foreign,
}

/// A confirmation the UI must show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    /// Run a root fix with pkexec.
    Root {
        /// Item title.
        title: String,
        /// The fix.
        fix: RootFix,
        /// A catalogue fix (or its undo): logged to `fixes.log` when its pkexec job ends.
        catalogue: Option<CatalogueRun>,
    },
    /// A step that writes your own calibration files: shown before it runs.
    Confirm {
        /// Title.
        title: String,
        /// What it does.
        detail: String,
        /// The command that runs.
        command: String,
        /// What runs on confirm.
        what: Confirm,
    },
    /// Play found a Steam that was started from inside SteamVR: ask before closing it (the Play dialog).
    RestartSteam {
        /// The tainted Steam.
        steam: crate::facts::TaintedSteam,
        /// The Play side that asked (true = game session, false = Work).
        vr: bool,
    },
    /// SIGINT one process.
    Interrupt {
        /// Pid.
        pid: u32,
        /// Name when it was seen.
        comm: String,
        /// Start time (clock ticks since boot) when the dialog opened: a reused pid has another one.
        start: Option<u64>,
    },
}

/// What a confirmed [`Pending::Confirm`] runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    /// Save the eye calibration refit of this run as the calibration.
    EyecalWrite(PathBuf),
    /// Save the controller gyro fit of these logs.
    ImufitSave(Vec<PathBuf>),
    /// A omarchyvrd request that stops or restarts something (the System page).
    Daemon(String),
}

/// A tool session (lens test, eye calibration): Monado started by the app, client in a terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTool {
    /// Which tool.
    pub tool: ToolId,
    /// Terminal title.
    pub title: String,
    /// Client script (a terminal is opened once the session is up), if any.
    pub script: Option<String>,
    /// The session is up (and its client terminal opened).
    pub opened: bool,
    /// Where the session records (walk test).
    pub record_dir: Option<PathBuf>,
}

/// Parsed documents for the Logs & History page.
#[derive(Debug, Clone, Default)]
pub struct Docs {
    /// INVENTORY section 4.
    pub history: Option<docs::Table>,
    /// INVENTORY section 5.1.
    pub open_issues: Option<docs::Table>,
    /// INVENTORY section 6.
    pub safety: Vec<(String, Vec<String>)>,
    /// LESSONS tables.
    pub lessons: Vec<(String, docs::Table)>,
    /// Where they were read from.
    pub source: PathBuf,
}

/// Environment lookup shared with threads.
pub type EnvFn = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// What the Logs page shows. Read in the background; the page only draws it.
#[derive(Debug, Clone, Default)]
pub struct LogView {
    /// (label, path) of the logs that exist.
    pub sources: Vec<(String, PathBuf)>,
    /// The list was read at least once.
    pub listed: bool,
    sources_at: Option<Instant>,
    /// The last lines of one log: its path and the lines.
    pub tail: Option<(PathBuf, Vec<String>)>,
    tail_at: Option<Instant>,
}

/// The dialog after a crash: the report and what still runs.
#[derive(Debug, Clone)]
pub struct Recovery {
    /// Report directory.
    pub dir: PathBuf,
    /// The report.
    pub report: crash::Report,
    /// More unseen reports than this one.
    pub more: usize,
}

/// The Desktop / WayVR row: the settings being edited, what is on disk, the conf.d check, the saved layout, the
/// live answers of `wayvrctl`.
#[derive(Debug, Clone, Default)]
pub struct WayvrView {
    /// The settings the row edits.
    pub settings: WayvrSettings,
    /// The settings file as read (None: there is none, the launcher's defaults apply).
    pub on_disk: Option<WayvrSettings>,
    /// Problems in the settings file.
    pub notes: Vec<String>,
    /// The conf.d check.
    pub lints: Vec<Lint>,
    /// The saved layout (None: no file; Err: unreadable).
    pub layout: Option<Result<Vec<Overlay>, String>>,
    /// The controls guide (numbered items of docs/WAYVR-CONTROLS.md).
    pub guide: Vec<String>,
    /// Apps inside WayVR (after "List apps").
    pub processes: Option<Vec<(String, u64)>>,
    /// The last `wayvrctl` result.
    pub last: String,
    /// Read at least once.
    pub loaded: bool,
    /// Saved while a desk session ran: it applies at the next start.
    pub pending_restart: bool,
    /// The app being typed into "Start an app".
    pub launch_exec: String,
    at: Option<Instant>,
}

/// The Room row.
#[derive(Debug, Clone, Default)]
pub struct RoomView {
    /// Maps, tag calibration, offset and its writer.
    pub state: RoomState,
    /// Read at least once.
    pub loaded: bool,
    /// The map name of the buttons.
    pub map_name: String,
    /// Mapping walk length.
    pub seconds: u32,
    at: Option<Instant>,
}

/// The Auto mode row: the daemon's last status (or why there is none).
#[derive(Debug, Clone, Default)]
pub struct AutoView {
    /// The last status, or the error (`not running`).
    pub status: Option<Result<crate::auto::AutoStatus, String>>,
    /// The last command's result.
    pub last: String,
    /// A pairing attempt: side, when it started, its state, the detail.
    pub pair: Option<(&'static str, Instant, crate::auto::PairState, String)>,
    /// The last pairing request per hand (left, right): at most one per 30 s.
    pub pair_at: [Option<Instant>; 2],
    at: Option<Instant>,
}

/// The applied fixes of `fixes.log` (newest first), read in the background.
#[derive(Debug, Clone, Default)]
pub struct FixesView {
    /// Entries.
    pub entries: Vec<fixlog::Entry>,
    /// Read at least once.
    pub loaded: bool,
    /// Read again on the next ask.
    pub stale: bool,
}

/// A room action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomAction {
    /// `omarchyvr-roommap build --source broker`.
    MapRoom,
    /// `omarchyvr-roommap relocalize --source broker --apply`.
    Recognise,
    /// `omarchyvr-roommap view --open`: the map as an HTML5 page in the browser.
    ViewMap,
    /// `omarchyvr-anchor calibrate`.
    TagCalibrate,
    /// `omarchyvr-anchor apply`.
    TagApply,
    /// `omarchyvr-anchor clear` (removes the offset, whoever wrote it).
    Clear,
}

/// "Report a problem".
#[derive(Debug, Clone, Default)]
pub struct ProblemReport {
    /// Being written.
    pub busy: bool,
    /// Add minidumps (raw memory; off unless the user ticks it).
    pub include_minidumps: bool,
    /// The last result: the file, or why it failed.
    pub result: Option<Result<PathBuf, String>>,
}

/// The app.
pub struct AppCore {
    /// Paths.
    pub paths: Paths,
    /// The machine.
    pub machine: Arc<dyn Machine>,
    /// Doctor options.
    pub opts: Opts,
    pub(crate) env: EnvFn,
    monitor: Option<Monitor>,
    follower: LogFollower,
    cache: Arc<MarkerCache>,
    /// Live state.
    pub live: Live,
    /// Facts.
    pub facts: Facts,
    /// Setup checklist.
    pub items: Vec<Item>,
    /// Current page.
    pub page: Page,
    /// Desk switches.
    pub toggles: DeskToggles,
    /// The user's OK to start over WayVR Xid 31 faults.
    pub consent_force: bool,
    /// Tools page options.
    pub tool_opts: ToolOptions,
    /// Running job.
    pub job: Option<Job>,
    /// Output of jobs (bounded).
    pub log: Vec<String>,
    /// The last microphone test's result line (plain words), and where its output starts in the log.
    pub mic_result: Option<String>,
    pub(crate) mic_test_from: Option<usize>,
    /// The same for the headset speaker test.
    pub speaker_result: Option<String>,
    pub(crate) speaker_test_from: Option<usize>,
    /// Status line.
    pub message: String,
    /// Confirmation to show.
    pub pending: Option<Pending>,
    /// Running tool session.
    pub session_tool: Option<SessionTool>,
    /// Onboarding.
    pub onboarding: OnboardingView,
    /// Autofix.
    pub autofix: AutofixView,
    /// The AI tab.
    pub ai: crate::ai::AiView,
    /// The AI tab's local model (omarchyvr-ai).
    pub localai: crate::localai::LocalAiView,
    /// The hand models download (Tracking page).
    pub hand_models: crate::handmodels::HandModelsView,
    /// The System page (omarchyvrd).
    pub system: crate::system::SystemView,
    /// The Record card (omarchyvr-record).
    pub record: crate::record::RecordView,
    /// `fixes.log`.
    pub fixes: FixesView,
    /// A catalogue fix whose job runs (job label, the fix): logged when the job ends.
    fix_logging: Option<(String, CatalogueRun)>,
    /// The onboarding asked for the Room page (the window shows it and clears this).
    pub room_requested: bool,
    /// Demo mode: the simulated live subtitles.
    pub demo_subtitles: bool,
    /// The eye laser as last switched from the app (WayVR has no query for it).
    pub eye_laser: Option<bool>,
    /// `~/.config/omarchyvr/eye.conf`: the gaze dot (visual effect) and the developer targeting switch.
    pub eye_settings: omarchyvr_config::eye::EyeSettings,
    /// The last headset restart's result.
    pub headset_reset: Option<crate::reset::ResetResult>,
    /// Where the running restart's output starts in the log.
    pub(crate) headset_reset_from: Option<usize>,
    /// The headset's live lens IPD in mm (info report), once read.
    pub lens_ipd: Option<f32>,
    /// The lens IPD was asked for since the last change.
    pub lens_ipd_asked: bool,
    /// The last lens move's result.
    pub ipd_result: Option<crate::ipdmotor::IpdResult>,
    /// Where the running lens move's output starts in the log.
    pub(crate) ipd_job_from: Option<usize>,
    /// Profiles page.
    pub profiles: Profiles,
    /// Remembered state.
    pub state: GuiState,
    /// Item to highlight on the Setup page.
    pub setup_focus: Option<String>,
    /// Result of the last walk test.
    pub walk_result: Option<String>,
    /// Where the docs are.
    pub docs_dir: PathBuf,
    docs: Option<Docs>,
    follow_up: Option<FollowUp>,
    pub(crate) wake: Arc<dyn Fn() + Send + Sync>,
    /// Demo/screenshot mode: nothing is ever started or signalled.
    pub demo: bool,
    /// The game-mode crash recovery check ran.
    pub gm_recovery_done: bool,
    worker: Worker,
    /// The Logs page data.
    pub logs: LogView,
    /// The crash dialog to show.
    pub recovery: Option<Recovery>,
    /// "Report a problem".
    pub problem: ProblemReport,
    /// The state to write (latest wins; written by the "state" task).
    state_slot: Arc<Mutex<Option<GuiState>>>,
    /// The launcher's plan per switch set (its parser looks at files: computed in the background).
    plans: std::collections::HashMap<DeskToggles, Result<DeskPlan, String>>,
    /// Plans computed since the last facts (older ones stay in use until they are redone: no flicker).
    plans_fresh: std::collections::HashSet<DeskToggles>,
    /// Crash reports on disk (seen or not).
    crash_total: usize,
    /// WayVR files.
    pub wayvr_paths: WayvrPaths,
    /// The Desktop / WayVR row.
    pub wayvr: WayvrView,
    /// Room tool files.
    pub room_paths: RoomPaths,
    /// The Room row.
    pub room: RoomView,
    /// The Auto mode row.
    pub auto: AutoView,
    /// The Ready check's VR games check (docs/GAMES-CHECK.md).
    pub gamecheck: crate::gamecheck::GameCheckView,
    /// The running game and the message after an xrizer crash (docs/GAME-MODE.md).
    pub game_watch: crate::gameruntime::GameWatch,
    /// The auto mode daemon's socket.
    pub auto_socket: PathBuf,
    /// Play, start at login, the hand test.
    pub play: crate::play::PlayView,
}

const LOG_MAX: usize = 3000;

impl AppCore {
    /// Build the app; collects everything once synchronously (tests, screenshots, `--self-test`).
    pub fn new(machine: Arc<dyn Machine>, paths: Paths, opts: Opts, env: EnvFn, store: Store) -> AppCore {
        let mut a = AppCore::new_background(machine, paths, opts, env, store);
        a.refresh_now(true);
        a
    }

    /// Build the app without collecting anything: the window opens right away and the first background round
    /// ([`AppCore::start_threads`]) fills it in (the desk preflight says "Checking this PC..." until then).
    pub fn new_background(machine: Arc<dyn Machine>, paths: Paths, opts: Opts, env: EnvFn, store: Store) -> AppCore {
        let state = GuiState::load(&paths.gui_state);
        let paths_for_rows = paths.clone();
        let env_for_rows = env.clone();
        let docs_dir = paths.repo.join("docs");
        let a = AppCore {
            follower: LogFollower::new(&paths.session_dir.join("monado.log")),
            paths,
            machine,
            opts,
            env,
            monitor: None,
            cache: Arc::new(MarkerCache::default()),
            live: Live::default(),
            facts: Facts::default(),
            items: Vec::new(),
            page: if state.onboarded && state.setup == crate::guistate::SetupStage::Done { state.page.unwrap_or(Page::Desk) } else { Page::Onboarding },
            toggles: state.toggles,
            consent_force: false,
            tool_opts: ToolOptions::default(),
            job: None,
            log: Vec::new(),
            mic_result: None,
            mic_test_from: None,
            speaker_result: None,
            speaker_test_from: None,
            message: String::new(),
            pending: None,
            session_tool: None,
            onboarding: OnboardingView::default(),
            autofix: AutofixView::new(Arc::new(crate::llm::Net)),
            ai: crate::ai::AiView::default(),
            localai: crate::localai::LocalAiView::default(),
            hand_models: Default::default(),
            system: crate::system::SystemView::default(),
            record: crate::record::RecordView::default(),
            fixes: FixesView::default(),
            fix_logging: None,
            room_requested: false,
            demo_subtitles: false,
            eye_laser: None,
            eye_settings: omarchyvr_config::eye::EyeSettings::load(&omarchyvr_config::eye::default_path()),
            headset_reset: None,
            headset_reset_from: None,
            lens_ipd: None,
            lens_ipd_asked: false,
            ipd_result: None,
            ipd_job_from: None,
            profiles: Profiles::new(store, None),
            state,
            setup_focus: None,
            walk_result: None,
            docs_dir,
            docs: None,
            follow_up: None,
            wake: Arc::new(|| {}),
            demo: false,
            gm_recovery_done: false,
            worker: Worker::inline(),
            logs: LogView::default(),
            recovery: None,
            problem: ProblemReport::default(),
            state_slot: Arc::default(),
            plans: Default::default(),
            plans_fresh: Default::default(),
            crash_total: 0,
            wayvr_paths: WayvrPaths::new(&paths_for_rows),
            wayvr: WayvrView::default(),
            room_paths: RoomPaths::new(&paths_for_rows),
            room: RoomView { map_name: "home".into(), seconds: 90, ..Default::default() },
            auto: AutoView::default(),
            gamecheck: Default::default(),
            game_watch: Default::default(),
            auto_socket: crate::auto::socket_path(&|k| env_for_rows(k)),
            play: Default::default(),
        };
        crate::crash::note("page", a.page.name());
        a
    }

    /// A demo app on a fixture machine (screenshots, tests): nothing is ever started.
    pub fn demo(running: bool, store: Store) -> AppCore {
        AppCore::demo_with(running, store, |m| m, false)
    }

    /// A demo app whose machine is wrapped (the UI tests slow it down and watch the UI thread with
    /// [`crate::probe::ProbeMachine`]). `background`: collect in the background like the real app (then call
    /// [`AppCore::start_threads`]); else right away. Its state (problem reports) goes to a temporary directory.
    pub fn demo_with(running: bool, store: Store, wrap: impl FnOnce(Arc<dyn Machine>) -> Arc<dyn Machine>, background: bool) -> AppCore {
        let (m, mut p) = if running { crate::demo::running_machine() } else { crate::demo::idle_machine() };
        p.state_dir = std::env::temp_dir().join(format!("pimax-demo-state-{}", std::process::id()));
        let mut opts = crate::demo::opts(&p);
        opts.config_root = store.root().to_path_buf();
        opts.cache_root = store.cache().to_path_buf();
        let env: EnvFn = Arc::new(|k: &str| match k {
            "HOME" => Some("/home/u".to_string()),
            "XDG_RUNTIME_DIR" => Some("/run/user/1000".to_string()),
            _ => None,
        });
        let machine = wrap(Arc::new(m));
        let mut a = if background {
            AppCore::new_background(machine, p, opts, env, store)
        } else {
            AppCore::new(machine, p, opts, env, store)
        };
        a.demo = true;
        a.eye_settings = Default::default();
        // demo mode answers Autofix itself: nothing leaves the machine
        let svc = pimax_device::checks::cpu::monado_service(&a.opts.monado_build);
        a.autofix = AutofixView::new(Arc::new(crate::llm::Demo { reply: crate::autofix::demo_reply(&svc) }));
        a.docs_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs");
        a.state = GuiState::default();
        a.toggles = DeskToggles::with_eye();
        a.page = Page::Desk;
        a
    }

    /// Repaint callback for background threads.
    pub fn set_wake(&mut self, f: Arc<dyn Fn() + Send + Sync>) {
        self.wake = f;
    }

    /// Start the background poller and the task threads (the real app; tests and screenshots stay synchronous).
    pub fn start_threads(&mut self) {
        self.monitor = Some(Monitor::start(
            self.machine.clone(),
            self.paths.clone(),
            self.opts.clone(),
            self.cache.clone(),
            self.wake.clone(),
        ));
        self.worker = Worker::threaded(self.wake.clone());
    }

    /// Same as [`AppCore::start_threads`].
    pub fn start_monitor(&mut self) {
        self.start_threads();
    }

    /// Run `f` in the background (inline in tests); its result is applied in [`AppCore::poll`]. With a `key`, a
    /// second task of that key does not start while one runs (returns false).
    pub fn task(&mut self, key: Option<&'static str>, f: impl FnOnce() -> Apply + Send + 'static) -> bool {
        let started = self.worker.run(key, f);
        if self.worker.is_inline() {
            self.apply_tasks();
        }
        started
    }

    /// Is a task of this key running?
    pub fn task_busy(&self, key: &str) -> bool {
        self.worker.busy(key)
    }

    fn apply_tasks(&mut self) -> bool {
        let done = self.worker.drain();
        let any = !done.is_empty();
        for a in done {
            a(self);
        }
        any
    }

    /// Collect now, on this thread.
    pub fn refresh_now(&mut self, doctor: bool) {
        self.live = collect_live(self.machine.as_ref(), &self.paths, &mut self.follower);
        self.facts = collect_facts(self.machine.as_ref(), &self.paths, &self.opts, &self.cache, Some(&self.facts).filter(|f| f.at != 0), doctor, true);
        self.after_facts();
    }

    /// Ask for fresh facts (background when the poller runs).
    pub fn refresh(&mut self, doctor: bool) {
        match &self.monitor {
            Some(m) => m.request(doctor, true),
            None => self.refresh_now(doctor),
        }
    }

    fn after_facts(&mut self) {
        self.items = checklist::build(&self.facts, &self.live, &self.paths);
        // a newer launcher starts eye tracking unless --no-eye
        self.toggles.eye_default_on = self.facts.caps.no_eye;
        self.adopt_start_argv();
        // a build may have appeared: the plans are worked out again (the old ones stay in use meanwhile)
        self.plans_fresh.clear();
        let serial = self
            .facts
            .doctor("headset")
            .filter(|c| c.level == omarchyvr_doctor::Level::Ok)
            .and_then(|_| self.facts.headset_serial.clone());
        if serial != self.profiles.serial {
            // re-reading the profiles is file I/O: in the background
            self.profiles_do(move |p| p.set_serial(serial));
        }
        // tool session ended outside the app (Monado gone): forget it
        if self.session_tool.as_ref().is_some_and(|s| s.opened) && self.live.service.is_none() && self.job.is_none() {
            self.session_tool = None;
        }
        self.setup_facts();
    }

    /// Drain the poller, the tasks and the job; true when something changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = self.apply_tasks();
        if self.demo {
            // demo pairing: "paired" after 6 s, nothing is sent
            if let Some(p) = self.auto.pair.as_mut() {
                if p.2 == crate::auto::PairState::Waiting && p.1.elapsed() > Duration::from_secs(6) {
                    p.2 = crate::auto::PairState::Paired;
                    p.3 = "demo: paired (nothing was sent)".into();
                    changed = true;
                }
            }
        }
        self.setup_poll();
        self.want_plan(self.effective_toggles());
        self.want_plan(self.effective_toggles().without_eye());
        self.want_plan(self.vr_toggles());
        changed |= self.poll_autostart();
        changed |= self.poll_gamemode_recovery();
        if let Some((side, _, crate::auto::PairState::Paired, _)) = &self.auto.pair {
            self.onboarding.paired[usize::from(*side == "right")] = true;
        }
        if self.state.page != Some(self.page) && self.state.onboarded {
            self.state.page = Some(self.page);
            crate::crash::note("page", self.page.name());
            self.write_state();
        }
        if !self.demo && !self.worker.busy("state") && self.state_slot.lock().map(|s| s.is_some()).unwrap_or(false) {
            self.start_state_writer();
        }
        if let Some(m) = &self.monitor {
            let mut got_facts = false;
            for u in m.poll() {
                changed = true;
                match u {
                    Update::Live(l) => {
                        self.live = *l;
                        self.watch_game();
                    }
                    Update::Facts(f) => {
                        self.facts = *f;
                        got_facts = true;
                    }
                }
            }
            if got_facts {
                self.after_facts();
            }
        }
        let mut done: Option<Result<(), String>> = None;
        if let Some(j) = &self.job {
            for e in j.poll() {
                changed = true;
                match e {
                    JobEvent::Line(l) => self.push_log(l),
                    JobEvent::Done(r) => done = Some(r),
                }
            }
        }
        if let Some(r) = done {
            let j = self.job.take().expect("job");
            crate::crash::note("job", "");
            self.job_finished(j.kind, &j.label, r);
        }
        changed
    }

    /// A background task failed in a way the user should know.
    pub fn internal_error(&mut self, what: &str, msg: &str) {
        log::error!("{what}: {msg}");
        self.message = format!("{what}: {msg}");
    }

    pub(crate) fn push_log(&mut self, l: String) {
        self.log.push(l);
        if self.log.len() > LOG_MAX {
            let n = self.log.len() - LOG_MAX;
            self.log.drain(..n);
        }
    }

    fn job_finished(&mut self, kind: JobKind, label: &str, r: Result<(), String>) {
        match &r {
            Ok(()) => self.message = format!("{label}: done"),
            Err(e) => self.message = format!("{label}: {e}"),
        }
        let m = self.message.clone();
        self.push_log(format!("== {m}"));
        match (&kind, &r) {
            (JobKind::Fix(_), Ok(())) => {
                if let Some(FollowUp::Setcap(svc)) = self.follow_up.take() {
                    // LESSONS row 7: the relink dropped cap_sys_nice; offer the setcap right away
                    self.pending = Some(Pending::Root {
                        title: format!("Restore cap_sys_nice on {}", svc.display()),
                        fix: checklist::setcap_fix(&svc),
                        catalogue: None,
                    });
                }
            }
            (JobKind::ToolSession(_), Ok(())) => {
                if let Some(s) = self.session_tool.clone() {
                    if let Some(script) = &s.script {
                        self.open_terminal(&s.title, script);
                    }
                    if let Some(st) = self.session_tool.as_mut() {
                        st.opened = true;
                    }
                }
            }
            (JobKind::ToolSession(_), Err(_)) => self.session_tool = None,
            (JobKind::DeskStart, Ok(())) => {
                self.state.toggles = self.toggles;
                self.note_start_result(label, &r);
                self.consent_force = false;
            }
            (JobKind::DeskStart, Err(_)) => self.note_start_result(label, &r),
            (JobKind::Other(k), _) if k == crate::handtest::KIND => self.hand_test_done(r.is_ok()),
            (JobKind::Stop, _) if self.live.service.is_none() => self.session_tool = None,
            _ => {}
        }
        if let JobKind::Other(k) = &kind {
            if k.starts_with("pair:") {
                if let Some(p) = self.auto.pair.as_mut() {
                    p.2 = if r.is_ok() { crate::auto::PairState::Paired } else { crate::auto::PairState::Timeout };
                    p.3 = if r.is_ok() { "paired".into() } else { "not paired (see the output)".into() };
                }
            }
            if k == crate::handmodels::KIND {
                self.hand_models_done(r.is_ok());
            }
            if k == MIC_TEST_KIND {
                self.mic_test_done(r.is_ok());
            }
            if k == SPEAKER_TEST_KIND {
                let from = self.speaker_test_from.take().unwrap_or(0).min(self.log.len());
                self.speaker_result = Some(test_result_from(&self.log[from..], "Speaker test:", r.is_ok()));
            }
            if k.starts_with("room:") {
                let k = k.clone();
                self.room_job_done(&k, r.is_ok());
            }
            if k == crate::reset::KIND {
                self.restart_headset_done(r.is_ok());
            }
            if k == crate::ipdmotor::KIND {
                self.ipd_motor_done(r.is_ok());
            }
            if k == crate::localai::KIND || k == crate::localai::TEST_KIND {
                let k = k.clone();
                self.localai_done(&k, r.is_ok());
            }
        }
        if !matches!(kind, JobKind::Fix(_)) {
            self.follow_up = None;
        }
        if let Some((l, run)) = self.fix_logging.take() {
            if l == label {
                let outcome = match &r {
                    Ok(()) => "done".to_string(),
                    Err(e) => format!("failed: {}", e.lines().next().unwrap_or("")),
                };
                self.log_fix(&run, &outcome);
            } else {
                self.fix_logging = Some((l, run));
            }
        }
        self.setup_job_finished(label, &r);
        self.onboarding_job_done(label, r.is_ok());
        self.refresh(true);
    }

    /// True while a job runs.
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// Start a job; false when it was not started (another one runs, or demo mode).
    pub(crate) fn spawn(&mut self, spec: JobSpec) -> bool {
        if self.busy() {
            self.message = "Another action is still running; wait for it to finish.".into();
            return false;
        }
        if self.demo {
            self.message = format!("demo mode: would run {}", spec.label);
            for s in &spec.steps {
                self.push_log(format!("(demo) {s:?}"));
            }
            return false;
        }
        self.push_log(format!("== {}", spec.label));
        log::info!("job: {}", spec.label);
        crate::crash::note("job", spec.label.clone());
        let j = job::spawn(spec, self.machine.clone(), self.wake.clone());
        if let Some(f) = &j.log_file {
            self.push_log(format!("(output also in {})", f.display()));
        }
        self.job = Some(j);
        true
    }

    /// The launcher binary the app calls.
    pub fn launcher_bin(&self) -> PathBuf {
        self.paths.launcher(&|x| self.facts.has(x))
    }

    /// The launcher's plan for the current switches (None while it is worked out in the background).
    pub fn plan(&self) -> Option<Result<DeskPlan, String>> {
        self.plan_for(&self.effective_toggles())
    }

    /// The launcher's option parser looks at the file system (which Monado build exists, the OpenVR library), so
    /// the window works it out in the background ([`AppCore::want_plan`]) and keeps the result per switch set; the
    /// inline mode (tests, `--self-test`) computes it right away.
    pub(crate) fn plan_for_toggles(&self, t: &DeskToggles) -> Option<Result<DeskPlan, String>> {
        self.plan_for(t)
    }

    fn plan_for(&self, t: &DeskToggles) -> Option<Result<DeskPlan, String>> {
        if let Some(p) = self.plans.get(t) {
            return Some(p.clone());
        }
        if self.worker.is_inline() {
            let env = self.env.clone();
            return Some(launcher::desk_plan(t, &move |k| env(k)));
        }
        None
    }

    /// Work out the plan of `t` in the background if it is not known or older than the last facts.
    pub fn want_plan(&mut self, t: DeskToggles) {
        if self.worker.is_inline() || self.plans_fresh.contains(&t) || self.worker.busy("plan") {
            return;
        }
        let env = self.env.clone();
        self.task(Some("plan"), move || {
            let p = launcher::desk_plan(&t, &move |k| env(k));
            Box::new(move |c: &mut AppCore| {
                c.plans.insert(t, p);
                c.plans_fresh.insert(t);
            })
        });
    }

    /// The preflight of a desk start with the current switches (as set: a switch the installed launcher lacks blocks
    /// the start instead of being dropped, so 6DoF never turns into a silent 3DoF start).
    pub fn preflight(&self) -> Preflight {
        self.preflight_for(&self.live, &self.facts, &self.effective_toggles())
    }

    /// No I/O: the build of the plan comes from the facts (every build the app knows, `PIMAX_MONADO_BUILD`
    /// included, is inspected in the background); an unknown one blocks until it was.
    pub(crate) fn preflight_for(&self, live: &Live, facts: &Facts, t: &DeskToggles) -> Preflight {
        let Some(plan) = self.plan_for(t) else {
            return preflight::checking("Reading the launcher's plan for these switches...");
        };
        let chosen = plan.as_ref().ok().and_then(|pl| facts.build(&pl.monado_build).cloned());
        preflight::evaluate(live, facts, t, &plan, chosen.as_ref(), &self.paths)
    }

    /// The Desk page state.
    pub fn phase(&self) -> DeskPhase {
        desk_phase(self.job.as_ref().map(|j| &j.kind), &self.live)
    }

    /// Run a launcher start in its own transient user scope (`systemd-run --user --scope`), so Monado and the desk
    /// clients never live in the app's cgroup: closing the app, or the desktop launcher cleaning up the app's scope or
    /// service, cannot stop them. Without systemd-run or a user manager the command runs as it is.
    pub fn own_scope(&self, c: Cmd, what: &str) -> Cmd {
        // found by the background check (no file system access on a click)
        let Some(sr) = self.facts.systemd_run.clone() else {
            return c;
        };
        if !self.facts.user_manager {
            return c;
        }
        let unit = format!("pimax-{what}-{}", self.machine.now_unix());
        let mut args: Vec<String> = ["--user", "--scope", "--quiet", "--collect", "--unit", unit.as_str(), "--description", "Pimax (started by the Pimax app)", "--"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        args.push(c.program.clone());
        args.extend(c.args.iter().cloned());
        Cmd {
            program: sr.display().to_string(),
            args,
            env: c.env,
            env_remove: c.env_remove,
            cwd: c.cwd,
            root: false,
        }
    }

    /// Environment additions for the launcher: the IPD of ipd.env (LESSONS row 18) and the supersampling factor
    /// (`XRT_COMPOSITOR_SCALE_PERCENTAGE`, which the launcher passes on to Monado).
    pub fn launcher_env(&self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = self.facts.ipd_mm.map(|mm| vec![("OXR_DEBUG_IPD_MM".to_string(), format!("{mm:.1}"))]).unwrap_or_default();
        if self.state.scale_pct != 100 {
            v.push(("XRT_COMPOSITOR_SCALE_PERCENTAGE".into(), self.state.scale_pct.to_string()));
        }
        v
    }

    /// Supersampling for the next start (50-200 %).
    pub fn set_scale(&mut self, pct: u16) {
        self.state.scale_pct = pct.clamp(50, 200);
        self.write_state();
    }

    /// Write `~/.config/omarchyvr/ipd.env` (`OXR_DEBUG_IPD_MM`, 58-72 mm) in the background: the next desk or VR start
    /// passes it to Monado (as `pimax-eye ipd --apply` does). Then, if no session owns the headset, move the lenses
    /// ([`AppCore::move_lenses`]).
    pub fn set_ipd(&mut self, mm: f32) {
        let mm = (mm * 10.0).round() / 10.0;
        if !(58.0..=72.0).contains(&mm) {
            self.message = format!("IPD {mm:.1} mm is outside 58-72 mm");
            return;
        }
        if self.demo {
            self.facts.ipd_mm = Some(mm);
            self.lens_ipd = Some(mm);
            self.message = format!("demo: IPD set to {mm:.1} mm");
            return;
        }
        let path = self.paths.ipd_env.clone();
        self.task(None, move || {
            let r = (|| -> std::io::Result<()> {
                if let Some(d) = path.parent() {
                    std::fs::create_dir_all(d)?;
                }
                let tmp = path.with_extension("env.tmp");
                std::fs::write(&tmp, format!("# written by the Pimax app\nOXR_DEBUG_IPD_MM={mm:.1}\n"))?;
                std::fs::rename(&tmp, &path)
            })();
            Box::new(move |c: &mut AppCore| {
                let saved = r.is_ok();
                c.message = match r {
                    Ok(()) => format!("IPD {mm:.1} mm saved; the next start uses it"),
                    Err(e) => format!("cannot write {}: {e}", path.display()),
                };
                c.refresh(false);
                // then the lenses themselves (the captured motor command), when no session owns the headset
                if saved && c.ipd_motor_blocker().is_none() {
                    c.move_lenses(mm);
                }
            })
        });
    }

    /// The start job for the current switches (`force` = the user's consent over WayVR Xid 31 faults).
    pub fn desk_start_job(&self, force: bool) -> Result<JobSpec, String> {
        let pf = self.preflight();
        match (&pf.gate, force) {
            (Gate::Blocked(r), _) => return Err(format!("not starting: {}", r.join("; "))),
            (Gate::NeedsConsent(r), false) => return Err(format!("needs your OK first: {}", r.join("; "))),
            _ => {}
        }
        let force = force && matches!(pf.gate, Gate::NeedsConsent(_));
        let t = self.effective_toggles();
        let c = self.own_scope(self.desk_start_cmd(&t, force), "desk");
        let mut steps = vec![
            Step::WaitNoMonado(Duration::from_secs(45)),
            Step::Guard("camera and eye chip still free", guards::devices_free(t.needs_cameras(), t.eye)),
        ];
        if force {
            steps.push(Step::Guard("still only WayVR Xid 31", guards::force_still_justified()));
        }
        steps.push(Step::Run(c));
        Ok(JobSpec {
            kind: JobKind::DeskStart,
            label: format!("Start desk mode ({})", launcher::desk_flags(&t).join(" ")),
            steps,
        })
    }

    /// `omarchyvr desk start <switches> [--force]` with the IPD of ipd.env and without stale experiment variables.
    pub(crate) fn desk_start_cmd(&self, t: &DeskToggles, force: bool) -> Cmd {
        let mut c = Cmd::with_args(self.launcher_bin().display().to_string(), launcher::desk_start_args(t, force));
        for (k, v) in self.launcher_env() {
            c = c.env(&k, v);
        }
        c.env_remove = self.facts.stale_env.iter().map(|(k, _)| k.clone()).collect();
        c
    }

    /// Start desk mode.
    pub fn start_desk(&mut self) {
        self.state.last_vr = false;
        match self.desk_start_job(self.consent_force) {
            Ok(j) => {
                self.spawn(j);
            }
            Err(e) => self.message = e,
        }
    }

    /// The stop job: `omarchyvr desk stop --grace 30` when desk clients or the broker run, else `omarchyvr vr stop --grace 30`.
    /// The launcher exits 2 whenever this boot's kernel log has any Xid line (also the WayVR ones), so its exit code is
    /// only reported; the next step checks that Monado is really gone.
    pub fn stop_job(&self) -> JobSpec {
        let args = if self.live.desk_side() { launcher::desk_stop_args() } else { launcher::vr_stop_args(false) };
        JobSpec {
            kind: JobKind::Stop,
            label: "Stop".into(),
            steps: vec![
                Step::RunAllowFail(Cmd::with_args(self.launcher_bin().display().to_string(), args)),
                Step::WaitNoMonado(Duration::from_secs(20)),
            ],
        }
    }

    /// The saved switches as a start uses them: camera hands only when this machine can run them (hands build, models,
    /// launcher), so a fresh default (hands on for Work) degrades to no `--hands` and no block.
    pub fn effective_toggles(&self) -> DeskToggles {
        let mut t = self.toggles;
        if self.hands_unavailable().is_some() {
            t.hands = false;
            t.hands_roles = false;
        }
        t
    }

    /// The switches of a game session (`omarchyvr vr start`): no desk clients; eye tracking follows its switch (visual only, no overlay in games).
    pub fn vr_toggles(&self) -> DeskToggles {
        let t = self.effective_toggles();
        let hands = self.state.hands_chosen && t.hands;
        let t = DeskToggles { passthrough: false, voice: false, hands, hands_roles: hands && t.hands_roles, ..t };
        if self.light_game() {
            // Light game mode: only what a game needs (docs/GAME-MODE.md); the camera hands stay as the user chose
            DeskToggles { eye: false, perf: false, anchor: false, ..t }
        } else {
            t
        }
    }

    /// The preflight of a game session.
    pub fn vr_preflight(&self) -> Preflight {
        let t = self.vr_toggles();
        let desk = self.preflight_for(&self.live, &self.facts, &t);
        let plan = self.plan_for(&t).and_then(|p| p.ok());
        preflight::for_vr(desk, plan.as_ref(), &self.facts)
    }

    /// The start job of a game session: wait for no Monado, re-check the cameras, `omarchyvr vr start ...` in its own
    /// scope. For Steam games through xrizer or native OpenXR apps; the app never launches the game.
    pub fn vr_start_job(&self, force: bool) -> Result<JobSpec, String> {
        let pf = self.vr_preflight();
        match (&pf.gate, force) {
            (Gate::Blocked(r), _) => return Err(format!("not starting: {}", r.join("; "))),
            (Gate::NeedsConsent(r), false) => return Err(format!("needs your OK first: {}", r.join("; "))),
            _ => {}
        }
        let force = force && matches!(pf.gate, Gate::NeedsConsent(_));
        let t = self.vr_toggles();
        let mut c = Cmd::with_args(self.launcher_bin().display().to_string(), launcher::vr_game_args(&t, force, self.light_game(), self.facts.next_game.as_deref()));
        for (k, v) in self.launcher_env() {
            c = c.env(&k, v);
        }
        c.env_remove = self.facts.stale_env.iter().map(|(k, _)| k.clone()).collect();
        let mut steps = vec![
            Step::WaitNoMonado(Duration::from_secs(45)),
            Step::Guard("camera still free", guards::devices_free(t.needs_cameras(), false)),
        ];
        if force {
            steps.push(Step::Guard("still only WayVR Xid 31", guards::force_still_justified()));
        }
        steps.push(Step::Run(self.own_scope(c, "vr")));
        Ok(JobSpec {
            kind: JobKind::DeskStart,
            label: format!("Start VR ({})", launcher::vr_game_args(&t, false, self.light_game(), self.facts.next_game.as_deref())[2..].join(" ")),
            steps,
        })
    }

    /// Start a game session.
    pub fn start_vr(&mut self) {
        self.state.last_vr = true;
        match self.vr_start_job(self.consent_force) {
            Ok(j) => {
                self.save_state();
                self.spawn(j);
            }
            Err(e) => self.message = e,
        }
    }

    /// Stop the session.
    pub fn stop(&mut self) {
        let j = self.stop_job();
        self.spawn(j);
    }


    /// Stop monado-service processes the launcher did not start (`omarchyvr vr stop --grace 30 --all`).
    pub fn stop_foreign(&mut self) {
        let j = JobSpec {
            kind: JobKind::Stop,
            label: "Stop other monado-service".into(),
            steps: vec![
                Step::RunAllowFail(Cmd::with_args(self.launcher_bin().display().to_string(), launcher::vr_stop_args(true))),
                Step::WaitNoMonado(Duration::from_secs(20)),
            ],
        };
        self.spawn(j);
    }

    /// LESSONS row 3: stop desk mode, wait for Monado to exit, start again without eye tracking. The new start goes
    /// through the same gate as a desk start (judged as if the session were already stopped), with `--force` only on
    /// the user's consent.
    pub fn restart_without_eye_job(&self) -> Result<JobSpec, String> {
        let t = self.effective_toggles().without_eye();
        let session: Vec<u32> = self.live.session.iter().filter_map(|s| s.pid).collect();
        let mut live = self.live.clone();
        live.service = None;
        for s in live.session.iter_mut() {
            s.alive = false;
        }
        live.monado_pids.retain(|p| !session.contains(p));
        live.foreign_monado.retain(|p| !session.contains(p));
        let mut facts = self.facts.clone();
        facts.camera_holders.retain(|h| !session.contains(&h.pid));
        facts.eye_holders.retain(|h| !session.contains(&h.pid));
        let force = match self.preflight_for(&live, &facts, &t).gate {
            Gate::Blocked(r) => return Err(format!("not restarting: {}", r.join("; "))),
            Gate::NeedsConsent(r) if !self.consent_force => {
                return Err(format!("tick 'Start anyway' on the Desk page first: {}", r.join("; ")))
            }
            Gate::NeedsConsent(_) => true,
            Gate::Ready => false,
        };
        let start = self.own_scope(self.desk_start_cmd(&t, force), "desk");
        let mut steps = vec![
            Step::RunAllowFail(Cmd::with_args(self.launcher_bin().display().to_string(), launcher::desk_stop_args())),
            Step::WaitNoMonado(Duration::from_secs(45)),
            Step::Guard("camera still free", guards::devices_free(t.needs_cameras(), false)),
        ];
        if force {
            steps.push(Step::Guard("still only WayVR Xid 31", guards::force_still_justified()));
        }
        steps.push(Step::Run(start));
        Ok(JobSpec {
            kind: JobKind::DeskStart,
            label: "Restart desk mode without eye tracking".into(),
            steps,
        })
    }

    /// Restart without eye tracking (or just switch it off when nothing runs).
    pub fn restart_without_eye(&mut self) {
        self.toggles.eye = false;
        self.save_state();
        if self.live.service.is_some() {
            match self.restart_without_eye_job() {
                Ok(j) => {
                    self.spawn(j);
                }
                Err(e) => self.message = e,
            }
        } else {
            self.message = "Eye tracking switched off for the next start.".into();
        }
    }

    /// Run a banner/tool action.
    pub fn act(&mut self, a: &Action) {
        match a {
            Action::StopSession => self.stop(),
            Action::StopForeignMonado => self.stop_foreign(),
            Action::InterruptPid { pid, comm } => {
                // the start time identifies the process when the user confirms (a reused pid has another one)
                let (m, pid, comm) = (self.machine.clone(), *pid, comm.clone());
                self.task(None, move || {
                    let start = procs::start_ticks(m.as_ref(), pid);
                    Box::new(move |c: &mut AppCore| c.pending = Some(Pending::Interrupt { pid, comm, start }))
                });
            }
            Action::Setup(id) => {
                self.page = Page::Setup;
                self.setup_focus = Some(id.clone());
            }
            Action::WithoutEye => self.restart_without_eye(),
        }
    }

    /// Confirmed: SIGINT to one pid, if it is still the same program and not monado-service.
    pub fn confirm_interrupt(&mut self) {
        let Some(Pending::Interrupt { pid, comm, start }) = self.pending.take() else { return };
        let dry = self.demo || !self.machine.can_signal() || crate::cmd::dry_run();
        let m = self.machine.clone();
        self.task(None, move || {
            let msg = match interrupt_pid(m.as_ref(), pid, &comm, start, dry) {
                Ok(x) | Err(x) => x,
            };
            log::info!("{msg}");
            Box::new(move |c: &mut AppCore| {
                c.message = msg;
                c.refresh(false);
            })
        });
    }

    /// Drop the pending confirmation.
    pub fn cancel_pending(&mut self) {
        self.pending = None;
    }

    /// An item by id.
    pub fn item(&self, id: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    /// Run a user fix.
    pub fn run_user_fix(&mut self, id: &str) {
        let Some(Fix::User(u)) = self.item(id).map(|i| i.fix.clone()) else { return };
        self.run_user(id, u);
    }

    fn run_user(&mut self, id: &str, u: UserFix) {
        if u.needs_idle && self.live.anything_running() {
            self.message = "Stop desk/VR first: this replaces binaries that a running session uses (LESSONS row 8).".into();
            return;
        }
        let follow = u.follow_up.clone();
        let spec = JobSpec {
            kind: JobKind::Fix(id.to_string()),
            label: format!("Fix: {}", self.item(id).map(|i| i.title.clone()).unwrap_or_else(|| id.to_string())),
            steps: u.steps.into_iter().map(Step::Run).collect(),
        };
        if self.spawn(spec) {
            self.follow_up = follow;
        }
    }

    /// Rebuild the desk Monado: `cmake -DPIMAX_CAM=ON .` (idempotent when it is on already) and `ninja` in its build
    /// directory, then the setcap dialog (the relink drops cap_sys_nice). Only while nothing runs.
    pub fn rebuild_desk(&mut self) {
        let b = self.paths.desk_build().clone();
        let u = checklist::rebuild_fix(&b, true);
        self.run_user("rebuild-desk", u);
    }

    /// Show the pkexec confirmation of a root fix.
    pub fn ask_root(&mut self, id: &str) {
        if let Some(i) = self.item(id) {
            if let Fix::Root(r) = &i.fix {
                self.pending = Some(Pending::Root { title: i.title.clone(), fix: r.clone(), catalogue: None });
            }
        }
    }

    /// Confirmed: run the pending root fix with pkexec.
    pub fn confirm_root(&mut self) {
        let Some(Pending::Root { title, fix, catalogue }) = self.pending.take() else { return };
        let step = self.onboarding.root_for.take();
        let Some(c) = fix.command() else {
            self.message = format!("{title}: copy the lines and run them in a terminal ({})", fix.pkexec.err().unwrap_or_default());
            return;
        };
        if !self.facts.polkit_agent {
            // no authentication agent: pkexec asks for the password in a terminal
            self.push_log(format!("== root fix in a terminal: {}", c.display()));
            if self.demo {
                self.message = format!("demo mode: would open a terminal for: {}", c.display().lines().next().unwrap_or(""));
                return;
            }
            let path = self.machine.env("PATH").unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into());
            let terminal = self.machine.env("TERMINAL");
            let log = self.paths.runtime_dir.join("omarchyvr-app-terminal-root.log");
            self.message = format!("{title}: opening a terminal...");
            self.task(None, move || {
                let msg = match crate::cmd::allow_start("a terminal") {
                    Err(e) => format!("{title}: {e}"),
                    Ok(()) => match term::pick_terminal(terminal.as_deref(), &path, &|p| p.is_file()) {
                        Some((bin, extra)) => match term::launch_root(&bin, &extra, &format!("Root fix: {title}"), &c, &log) {
                            Ok(_) => format!("{title}: opened in a terminal (pkexec asks for your password there); Re-check afterwards"),
                            Err(e) => format!("{title}: {e}"),
                        },
                        None => format!("{title}: no terminal found; copy the lines and run them yourself"),
                    },
                };
                let opened = msg.contains("opened in a terminal");
                Box::new(move |a: &mut AppCore| {
                    a.message = msg;
                    if let (true, Some(run)) = (opened, catalogue) {
                        // the result is in the terminal: logged as opened, so it can still be undone from the list
                        a.log_fix(&run, "opened in a terminal (result not checked)");
                    }
                })
            });
            return;
        }
        let label = format!("Root fix: {title} (pkexec)");
        if self.spawn(JobSpec { kind: JobKind::Fix(format!("root: {title}")), label: label.clone(), steps: vec![Step::Run(c)] }) {
            if let Some(run) = catalogue {
                self.fix_logging = Some((label.clone(), run));
            }
            if let Some(s) = step {
                self.onboarding.running = Some((s, label));
            }
        }
    }

    /// Run a catalogue fix (or its undo): user fixes as a job, root fixes through the pkexec dialog (nothing runs
    /// before the user confirms it there), reboots and empty undos are only shown.
    pub fn run_catalogue(&mut self, run: CatalogueRun, title: &str) {
        match findings::action(&run, &self.launcher_bin()) {
            FixAction::User(steps) => {
                let label = format!("{}: {title}", if run.undo { "Undo" } else { "Fix" });
                let spec = JobSpec { kind: JobKind::Fix(format!("catalogue:{}", run.planned.fix.id)), label: label.clone(), steps: steps.into_iter().map(Step::Run).collect() };
                if self.spawn(spec) {
                    self.fix_logging = Some((label, run));
                }
            }
            FixAction::Root(fix) => {
                let title = if run.undo { format!("Undo: {title}") } else { title.to_string() };
                self.pending = Some(Pending::Root { title, fix, catalogue: Some(run) });
            }
            FixAction::Manual(why, lines) => {
                for l in &lines {
                    self.push_log(format!("(to run yourself) {l}"));
                }
                self.message = why;
            }
        }
    }

    /// Append a fixes.log line (in the background; nothing in demo mode).
    pub(crate) fn log_fix(&mut self, run: &CatalogueRun, outcome: &str) {
        if self.demo {
            return;
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let line = run.log_line(now, outcome);
        let path = fixlog::path(&self.paths.state_dir);
        self.task(None, move || {
            let r = fixlog::append(&path, &line);
            Box::new(move |c: &mut AppCore| {
                if let Err(e) = r {
                    c.message = format!("cannot write {}: {e}", path.display());
                }
                c.fixes.stale = true;
            })
        });
    }

    /// Keep the applied fixes list fresh (read in the background when asked first and after a fix).
    pub fn want_fixes(&mut self) {
        if self.demo {
            // demo mode writes no log: two fixes from earlier, to show the list
            if !self.fixes.loaded && self.facts.at != 0 {
                self.fixes.entries = fixlog::demo_entries(self.facts.at, &self.opts.monado_build);
                self.fixes.loaded = true;
            }
            return;
        }
        if (self.fixes.loaded && !self.fixes.stale) || self.task_busy("fixes") {
            return;
        }
        self.fixes.stale = false;
        let path = fixlog::path(&self.paths.state_dir);
        self.task(Some("fixes"), move || {
            let v = fixlog::read(&path);
            Box::new(move |c: &mut AppCore| {
                c.fixes.entries = v;
                c.fixes.loaded = true;
            })
        });
    }

    /// Undo a logged fix (an undo entry applies the fix again).
    pub fn undo_logged(&mut self, i: usize) {
        let Some(e) = self.fixes.entries.get(i).cloned() else { return };
        let Some(planned) = e.planned() else {
            self.message = format!("{} is not in this app's fix catalogue any more", e.id);
            return;
        };
        let title = e.id.clone();
        self.run_catalogue(CatalogueRun { planned, undo: !e.is_undo() }, &title);
    }

    /// Can a tool start now?
    pub fn tool_availability(&self, t: ToolId) -> Availability {
        tools::availability(t, &self.live, &self.facts, &self.tool_opts, &self.paths)
    }

    fn stamp(&self) -> String {
        omarchyvr_config::store::stamp(self.machine.now_unix())
    }

    /// Start a tool.
    pub fn run_tool(&mut self, t: ToolId) {
        let a = self.tool_availability(t);
        if !a.ready() {
            self.message = format!("{}: {}", t.title(), a.blockers.iter().map(|b| b.text.clone()).collect::<Vec<_>>().join(" "));
            return;
        }
        if self.busy() {
            self.message = "Another action is still running.".into();
            return;
        }
        let stamp = self.stamp();
        match tools::launch(t, &self.live, &self.facts, &self.tool_opts, &self.paths, &stamp) {
            Launch::Terminal { title, script } => self.open_terminal(&title, &script),
            Launch::Job(j) => {
                self.spawn(j);
            }
            Launch::Session { mut start, title, script } => {
                // the launcher start of the session in its own scope (not the mkdir before it)
                let launcher = self.launcher_bin().display().to_string();
                for s in start.steps.iter_mut() {
                    if let Step::Run(c) = s {
                        if c.program == launcher {
                            *c = self.own_scope(c.clone(), "tool");
                        }
                    }
                }
                let record_dir = (t == ToolId::WalkTest && self.tool_opts.walk_euroc).then(|| tools::walk_dir(&self.paths, &stamp));
                if self.spawn(start) {
                    self.session_tool = Some(SessionTool { tool: t, title, script, opened: false, record_dir });
                    self.walk_result = None;
                    // the consent was for this start
                    self.tool_opts.consent_force = false;
                }
            }
        }
    }

    /// Stop a tool session (`omarchyvr vr stop --grace 30` / `omarchyvr desk stop --grace 30`).
    pub fn stop_tool_session(&mut self) {
        self.stop();
    }

    /// End the walk test: keep its 6DoF result, then stop the session.
    pub fn finish_walk(&mut self) {
        let s = &self.live.log.sixdof;
        let dur = self.live.service.as_ref().and_then(|x| x.age).map(procs::fmt_age).unwrap_or_else(|| "?".into());
        let rec = self
            .session_tool
            .as_ref()
            .and_then(|t| t.record_dir.clone())
            .map(|d| format!("; recording {} (replay: analysis/tools/run_slam.sh replay {})", d.display(), d.display()))
            .unwrap_or_default();
        self.walk_result = Some(format!(
            "walk of {dur}: final state {}, {} acquired, {} lost, {} tracker resets, {} frame sets, {} IMU samples{}{}",
            s.state.label(),
            s.acquisitions,
            s.losses,
            s.resets,
            s.frame_sets,
            s.imu_samples,
            s.correction.map(|(mm, deg)| format!(", last correction {mm:.1} mm / {deg:.2} deg")).unwrap_or_default(),
            rec
        ));
        self.stop();
    }

    /// Steam runtime switch (`xrizer`, `steamvr`, `status`).
    pub fn runtime(&mut self, what: &str) {
        if !matches!(what, "xrizer" | "steamvr" | "status") {
            return;
        }
        let j = tools::runtime_job(&self.paths, &self.facts, what);
        self.spawn(j);
    }

    /// "Sound in headset" (`on`, `off`) and "Headset microphone" (`mic-on`, `mic-off`).
    pub fn sound_headset(&mut self, what: &str) {
        if !matches!(what, "on" | "off" | "mic-on" | "mic-off") {
            return;
        }
        let j = tools::audio_job(&self.paths, &self.facts, what);
        self.spawn(j);
    }

    /// "Test microphone": record 3 s from the headset microphone, show the level, play it back (`omarchyvr vr audio
    /// mic-test`).
    pub fn mic_test(&mut self) {
        if self.demo {
            self.mic_result = Some("headset microphone, level -31 dBFS (peak -9 dBFS, 3.0 s, 2 ch): good".into());
            return;
        }
        self.mic_result = None;
        let j = tools::audio_job(&self.paths, &self.facts, "mic-test");
        if self.spawn(j) {
            self.mic_test_from = Some(self.log.len());
        }
    }

    pub(crate) fn mic_test_done(&mut self, ok: bool) {
        let from = self.mic_test_from.take().unwrap_or(0).min(self.log.len());
        self.mic_result = Some(mic_result_from(&self.log[from..], ok));
    }

    /// "Test headset sound": a short chime on the headset speaker only (`omarchyvr vr audio speaker-test`).
    pub fn speaker_test(&mut self) {
        if self.demo {
            self.speaker_result = Some("played on the headset speaker; you should have heard two soft notes".into());
            return;
        }
        self.speaker_result = None;
        let j = tools::audio_job(&self.paths, &self.facts, "speaker-test");
        if self.spawn(j) {
            self.speaker_test_from = Some(self.log.len());
        }
    }

    /// "Game mode" (`on`, `off`) and the "Compositor time budget" (`auto`, `6`, `8`).
    pub fn game_mode(&mut self, on: bool) {
        let j = tools::gamemode_job(&self.paths, &self.facts, &[if on { "on" } else { "off" }]);
        self.spawn(j);
    }

    pub fn comp_budget(&mut self, b: &str) {
        if !matches!(b, "auto" | "6" | "8") {
            return;
        }
        let j = tools::gamemode_job(&self.paths, &self.facts, &["budget", b]);
        self.spawn(j);
    }

    /// "Brightness": the image level (`standard`, `bright`, `brighter`; next start) or the backlight in percent (now).
    pub fn image_brightness(&mut self, v: &str) {
        let j = tools::display_job(&self.paths, &self.facts, &["image", v]);
        self.spawn(j);
    }

    pub fn backlight(&mut self, pct: u8) {
        let p = pct.to_string();
        let j = tools::display_job(&self.paths, &self.facts, &["backlight", &p]);
        self.spawn(j);
    }

    /// Crash recovery: the local AI was paused (an older version also Sunshine) by a session that is gone (app or PC crashed): start
    /// them again, once, when the app has looked and nothing runs.
    pub(crate) fn poll_gamemode_recovery(&mut self) -> bool {
        if self.demo || self.gm_recovery_done || self.facts.at == 0 {
            return false;
        }
        self.gm_recovery_done = true;
        if self.live.gm_paused.is_empty() || self.live.service.is_some() || !self.live.monado_pids.is_empty() || self.busy() {
            return false;
        }
        let j = tools::gamemode_job(&self.paths, &self.facts, &["restore"]);
        self.spawn(j)
    }

    /// "Set up Steam for VR games" (`on`) and its undo (`off`).
    pub fn steam_setup(&mut self, what: &str) {
        if !matches!(what, "on" | "off" | "status") {
            return;
        }
        let j = tools::steam_setup_job(&self.paths, &self.facts, what);
        self.spawn(j);
    }

    /// The Steam launch options line.
    pub fn steam_options(&self) -> String {
        tools::steam_options_line(&self.paths)
    }

    /// Open a script in the user's terminal (in the background; the status line says how it went).
    pub fn open_terminal(&mut self, title: &str, script: &str) {
        let path = self.machine.env("PATH").unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into());
        let terminal = self.machine.env("TERMINAL");
        self.push_log(format!("== terminal: {}", script.lines().next().unwrap_or("")));
        let title = title.to_string();
        if self.demo {
            self.message = format!("{title}: opened in a terminal (demo: nothing started)");
            return;
        }
        let script = script.to_string();
        let log = self.paths.runtime_dir.join(format!("omarchyvr-app-terminal-{}.log", title.replace(|c: char| !c.is_ascii_alphanumeric(), "-")));
        self.message = format!("{title}: opening a terminal...");
        self.task(None, move || {
            let r = crate::cmd::allow_start("a terminal").and_then(|()| {
                let (bin, extra) = term::pick_terminal(terminal.as_deref(), &path, &|p| p.is_file())
                    .ok_or_else(|| "no terminal found ($TERMINAL, foot, alacritty, kitty)".to_string())?;
                let cmd = term::terminal_cmd(&bin, &extra, &title, &script);
                term::launch(&cmd, &script, &log)
            });
            let msg = match r {
                Ok(_) => format!("{title}: opened in a terminal"),
                Err(e) => format!("{title}: {e}"),
            };
            log::info!("{msg}");
            Box::new(move |c: &mut AppCore| c.message = msg)
        });
    }

    /// Keep the Logs page data fresh (call it while the page shows): the list of logs every 3 s, the tail of the
    /// selected one every second while following. All reads happen in the background.
    pub fn want_logs(&mut self, selected: usize, follow: bool) {
        let now = Instant::now();
        if self.logs.sources_at.is_none_or(|t| now.duration_since(t) > Duration::from_secs(3)) {
            self.logs.sources_at = Some(now);
            let (m, p) = (self.machine.clone(), self.paths.clone());
            self.task(Some("log-sources"), move || {
                let v = log_sources(m.as_ref(), &p);
                Box::new(move |c: &mut AppCore| {
                    c.logs.sources = v;
                    c.logs.listed = true;
                })
            });
        }
        let Some((_, path)) = self.logs.sources.get(selected).cloned() else { return };
        let fresh = match (&self.logs.tail, self.logs.tail_at) {
            (Some((p, _)), Some(t)) => *p == path && (!follow || now.duration_since(t) < Duration::from_secs(1)),
            _ => false,
        };
        let waiting = self.logs.tail.as_ref().is_some_and(|(p, _)| *p != path) && self.logs.tail_at.is_some_and(|t| now.duration_since(t) < Duration::from_secs(1));
        if !fresh && !waiting {
            self.logs.tail_at = Some(now);
            let m = self.machine.clone();
            self.task(Some("tail"), move || {
                let lines = tail(m.as_ref(), &path, 600);
                Box::new(move |c: &mut AppCore| c.logs.tail = Some((path, lines)))
            });
        }
    }

    /// INVENTORY and LESSONS, parsed once in the background; None while they load.
    pub fn docs(&mut self) -> Option<&Docs> {
        if self.docs.is_none() && !self.worker.busy("docs") {
            let dir = self.docs_dir.clone();
            self.task(Some("docs"), move || {
                let d = load_docs(&dir);
                Box::new(move |c: &mut AppCore| c.docs = Some(d))
            });
        }
        self.docs.as_ref()
    }

    /// The app's colours (Appearance setting): remembered with the other app switches.
    pub fn set_colours(&mut self, c: crate::colors::Colours) {
        if self.state.colours != c {
            self.state.colours = c;
            self.write_state();
        }
    }

    /// Remember the switches.
    pub fn save_state(&mut self) {
        self.state.toggles = self.toggles;
        self.write_state();
    }

    /// Write the app state in the background (the newest state wins; nothing in demo mode).
    pub(crate) fn write_state(&mut self) {
        if self.demo {
            return;
        }
        if let Ok(mut s) = self.state_slot.lock() {
            *s = Some(self.state);
        }
        if !self.worker.busy("state") {
            self.start_state_writer();
        }
    }

    fn start_state_writer(&mut self) {
        let (slot, path) = (self.state_slot.clone(), self.paths.gui_state.clone());
        self.task(Some("state"), move || {
            let mut err = None;
            loop {
                let next = slot.lock().ok().and_then(|mut s| s.take());
                let Some(st) = next else { break };
                if let Err(e) = st.save(&path) {
                    err = Some(format!("cannot save {}: {e}", path.display()));
                }
            }
            Box::new(move |c: &mut AppCore| {
                if let Some(e) = err {
                    c.message = e;
                }
            })
        });
    }

    /// Write the newest state now (the window is closing).
    pub fn flush_state(&mut self) {
        if self.demo {
            return;
        }
        let next = self.state_slot.lock().ok().and_then(|mut s| s.take());
        if let Some(st) = next {
            let _ = st.save(&self.paths.gui_state);
        }
    }

    /// Change the profiles in the background (their files); the result replaces the page state.
    pub fn profiles_do(&mut self, f: impl FnOnce(&mut Profiles) + Send + 'static) {
        if self.worker.is_inline() {
            f(&mut self.profiles);
            return;
        }
        let mut p = self.profiles.clone();
        self.task(None, move || {
            f(&mut p);
            Box::new(move |c: &mut AppCore| {
                let keep = (c.profiles.confirm_mismatch, c.profiles.link_legacy);
                c.profiles = p;
                (c.profiles.confirm_mismatch, c.profiles.link_legacy) = keep;
            })
        });
    }

    /// Show the oldest crash report the user has not seen (read in the background at start).
    pub fn check_crashes(&mut self) {
        let state = self.paths.state_dir.clone();
        self.task(Some("crashes"), move || {
            let total = crash::list(&state).into_iter().filter(|(_, r, _)| r.kind.fatal()).count();
            let mut v = crash::unseen(&state);
            Box::new(move |c: &mut AppCore| {
                c.crash_total = total;
                if let Some((dir, report)) = v.pop() {
                    c.recovery = Some(Recovery { dir, report, more: v.len() });
                }
            })
        });
    }

    /// Crash reports of this user (written by the app; read at start).
    pub fn crash_count(&self) -> usize {
        self.crash_total
    }

    /// The crash dialog was closed: the report counts as seen.
    pub fn dismiss_recovery(&mut self) {
        if let Some(r) = self.recovery.take() {
            // (demo mode keeps its reports in a temporary directory)
            let dir = r.dir.clone();
            self.task(None, move || {
                let _ = crash::mark_seen(&dir);
                Box::new(|_: &mut AppCore| {})
            });
            if r.more > 0 {
                self.check_crashes();
            }
        }
    }

    /// What still runs after the crash of the dialog.
    pub fn orphans(&self) -> Vec<orphans::Orphan> {
        let job = self.recovery.as_ref().and_then(|r| r.report.ctx("job").map(str::to_string));
        orphans::find(&self.live, &self.facts, job.as_deref())
    }

    /// The summary of a problem report: version, system, what runs, the verdicts.
    pub fn problem_summary(&self) -> String {
        let mut o = String::new();
        for (k, v) in crash::system() {
            o += &format!("{k}: {v}\n");
        }
        o += &format!("page: {}\n", self.page.name());
        o += &format!("job: {}\n", self.job.as_ref().map(|j| j.label.clone()).unwrap_or_else(|| "none".into()));
        o += &format!("message: {}\n", self.message);
        o += &format!(
            "monado: {}\n",
            self.live.service.as_ref().map(|s| format!("pid {} (eye {}, 6dof {}, ctrl {})", s.pid, s.eye, s.sixdof, s.ctrl_track)).unwrap_or_else(|| "stopped".into())
        );
        let alive: Vec<String> = self.live.session.iter().filter(|p| p.alive).map(|p| format!("{} {}", p.name, p.pid.unwrap_or(0))).collect();
        o += &format!("session processes: {}\n", if alive.is_empty() { "none".into() } else { alive.join(", ") });
        o += &format!("foreign monado: {:?}\nstray clients: {:?}\n", self.live.foreign_monado, self.facts.stray_clients);
        o += &format!("desk switches: {}\n", launcher::desk_flags(&self.toggles).join(" "));
        let (ok, warn, fail) = checklist::counts(&self.items);
        o += &format!("setup: {ok} ok, {warn} warn, {fail} fail\n");
        o += &format!("headset: {}, eye chip: {}\n", self.facts.headset, self.facts.tobii);
        o += &format!("kernel log: {}\n", self.facts.kernel_error.clone().unwrap_or_else(|| format!("{} GPU events", self.facts.kernel.len())));
        o += &format!("6dof: {:?}\n", self.live.log.sixdof);
        o += "last lines of the job output:\n";
        for l in self.log.iter().rev().take(40).collect::<Vec<_>>().into_iter().rev() {
            o += &format!("  {l}\n");
        }
        o
    }

    /// Write a problem report in the background (redacted `.tar.gz` under `~/.local/state/omarchyvr/reports`).
    pub fn report_problem(&mut self) {
        if self.problem.busy {
            return;
        }
        let pf = self.preflight();
        let mut preflight = format!("gate: {:?}\n", pf.gate);
        for f in &pf.findings {
            preflight += &format!("{:?} {} (row {:?}): {} | {}\n", f.sev, f.id, f.row, f.title, f.detail);
        }
        let mut checklist = String::new();
        for i in &self.items {
            checklist += &format!("{:?} {}: {} | {}\n", i.level, i.id, i.title, i.detail);
        }
        let kernel = if self.facts.kernel.is_empty() {
            self.facts.kernel_error.clone().unwrap_or_else(|| "no GPU kernel events this boot".into())
        } else {
            self.facts.kernel.iter().map(|e| e.summary()).collect::<Vec<_>>().join("\n")
        };
        let mut serials: Vec<String> = Vec::new();
        serials.extend(self.facts.headset_serial.clone());
        serials.extend(self.facts.tobii_serial.clone());
        serials.extend(self.profiles.serial.clone());
        let env = self.env.clone();
        // demo data has no session logs of its own (the real ones stay out of a demo report)
        let session_dir = (!self.demo).then(|| self.paths.session_dir.clone());
        let input_base = bundle::Input {
            state_dir: self.paths.state_dir.clone(),
            out_dir: self.paths.state_dir.join("reports"),
            stamp: crate::applog::now_utc().replace([':', '-'], "").split('.').next().unwrap_or("").to_string() + "Z",
            summary: self.problem_summary(),
            doctor: self.facts.report.as_ref().map(|r| (r.render(), r.to_json().render())),
            checklist,
            preflight,
            kernel,
            session_logs: Vec::new(),
            include_minidumps: self.problem.include_minidumps,
            redactor: crate::redact::Redactor::default(),
        };
        self.problem.busy = true;
        self.problem.result = None;
        self.task(Some("report"), move || {
            let mut input = input_base;
            input.redactor = crate::redact::Redactor::from_system(&|k| env(k), &serials);
            if let Some(dir) = &session_dir {
                input.session_logs = ["monado.log", "monado.log.prev", "wayvr.log", "passthrough.log", "broker.log", "ctrltrack.log", "voice.log"]
                    .iter()
                    .map(|f| dir.join(f))
                    .filter(|p| p.is_file())
                    .collect();
            }
            let r = bundle::build(&input);
            match &r {
                Ok(p) => log::info!("problem report written: {}", p.display()),
                Err(e) => log::error!("problem report failed: {e}"),
            }
            Box::new(move |c: &mut AppCore| {
                c.problem.busy = false;
                c.problem.result = Some(r);
            })
        });
    }

    /// Keep the Desktop / WayVR row fresh while it is open: the settings file, the conf.d check, the saved layout and
    /// the controls guide, read in the background every 3 s.
    pub fn want_wayvr(&mut self) {
        if self.wayvr.at.is_some_and(|t| t.elapsed() < Duration::from_secs(3)) {
            return;
        }
        self.wayvr.at = Some(Instant::now());
        let (m, w) = (self.machine.clone(), self.wayvr_paths.clone());
        self.task(Some("wayvr"), move || {
            let (on_disk, notes) = match m.read_str(&w.settings) {
                Some(t) => {
                    let (s, n) = WayvrSettings::parse(&t);
                    (Some(s), n)
                }
                None => (None, Vec::new()),
            };
            let lints = wayvr::lint(&wayvr::read_conf_d(m.as_ref(), &w.conf_d));
            let layout = m.read(&w.saved_state).map(|b| wayvr::parse_layout(&String::from_utf8_lossy(&b)));
            let guide: Vec<String> = m
                .read_str(&w.guide)
                .map(|t| t.lines().filter(|l| l.chars().next().is_some_and(|c| c.is_ascii_digit())).map(|l| l.replace("**", "")).collect())
                .unwrap_or_default();
            Box::new(move |c: &mut AppCore| {
                if !c.wayvr.loaded {
                    c.wayvr.settings = on_disk.clone().unwrap_or_default();
                }
                c.wayvr.on_disk = on_disk;
                c.wayvr.notes = notes;
                c.wayvr.lints = lints;
                c.wayvr.layout = layout;
                c.wayvr.guide = guide;
                c.wayvr.loaded = true;
            })
        });
    }

    /// The screens of the saved layout (captions placement, the row summary).
    pub fn wayvr_screens(&self) -> Vec<String> {
        match &self.wayvr.layout {
            Some(Ok(l)) => wayvr::screens(l),
            _ => Vec::new(),
        }
    }

    /// Save the WayVR settings (validated; the next desk start applies them).
    pub fn save_wayvr(&mut self) {
        let bad = self.wayvr.settings.validate();
        if !bad.is_empty() {
            self.message = format!("WayVR settings not saved: {}", bad.join("; "));
            return;
        }
        let s = self.wayvr.settings.clone();
        if self.demo {
            self.wayvr.on_disk = Some(s);
            self.message = "demo: WayVR settings saved".into();
            return;
        }
        let path = self.wayvr_paths.settings.clone();
        let running = self.live.alive("wayvr");
        self.task(None, move || {
            let r = wayvr::save_settings(&path, &s);
            Box::new(move |c: &mut AppCore| match r {
                Ok(()) => {
                    c.wayvr.on_disk = Some(s);
                    c.wayvr.pending_restart = running;
                    c.message = if running {
                        "WayVR settings saved: restart desk mode to apply them".into()
                    } else {
                        "WayVR settings saved: the next desk start applies them".into()
                    };
                }
                Err(e) => c.message = format!("cannot save {}: {e}", path.display()),
            })
        });
    }

    /// Run a `wayvrctl` command against the running WayVR.
    pub fn wayvr_ctl(&mut self, c: Ctl) {
        if !self.live.alive("wayvr") && !self.demo {
            self.message = "WayVR is not running (start desk mode first)".into();
            return;
        }
        if self.demo {
            self.wayvr.last = format!("demo: would run wayvrctl {}", c.args().join(" "));
            if c == Ctl::ProcessList {
                self.wayvr.processes = Some(vec![("Terminal".into(), 1)]);
            }
            return;
        }
        let ctl = self.wayvr_paths.ctl.clone();
        let label = c.label();
        self.wayvr.last = format!("{label}...");
        self.task(Some("wayvrctl"), move || {
            let r = wayvr::run_ctl(&ctl, &c);
            log::info!("wayvrctl {}: {}", c.args().join(" "), match &r { Ok(_) => "ok".to_string(), Err(e) => e.clone() });
            Box::new(move |a: &mut AppCore| match (r, &c) {
                (Ok(out), Ctl::ProcessList) => {
                    let p = wayvr::parse_processes(&out);
                    a.wayvr.last = format!("{} app(s) inside WayVR", p.len());
                    a.wayvr.processes = Some(p);
                }
                (Ok(_), _) => a.wayvr.last = format!("{label}: sent (see wayvr.log for WayVR's side)"),
                (Err(e), _) => a.wayvr.last = format!("{label}: {e}"),
            })
        });
    }

    /// "Use controllers in Work": saved at once in the WayVR settings (the next desk start writes it into WayVR's
    /// conf.d; unsaved edits of the WayVR page stay unsaved) and sent to a running WayVR (`wayvrctl controllers on|off`).
    pub fn set_controllers_in_work(&mut self, on: bool) {
        self.wayvr.settings.controllers_in_work = on;
        if self.live.alive("wayvr") {
            self.wayvr_ctl(Ctl::Controllers(on));
        }
        let mut s = self.wayvr.on_disk.clone().unwrap_or_default();
        s.controllers_in_work = on;
        if self.demo {
            self.wayvr.on_disk = Some(s);
            return;
        }
        let path = self.wayvr_paths.settings.clone();
        self.task(None, move || {
            let r = wayvr::save_settings(&path, &s);
            Box::new(move |c: &mut AppCore| match r {
                Ok(()) => {
                    if let Some(d) = c.wayvr.on_disk.as_mut() {
                        d.controllers_in_work = on;
                    } else {
                        c.wayvr.on_disk = Some(s);
                    }
                    c.message = format!("{}: saved for every desk start", if on { "Controllers on in Work" } else { "Hands only in Work" });
                }
                Err(e) => c.message = format!("cannot save {}: {e}", path.display()),
            })
        });
    }

    /// "Controllers" (when picked up / always / off, docs/CONTROLLERS-INHAND.md) for Work (`play == false`) or Play:
    /// saved at once (the next start passes `--controllers`, start-argv.txt keeps it for restarts) and, when a session
    /// of that side runs with a Monado that knows it, changed there too (`$XDG_RUNTIME_DIR/pimax-ctrl-inhand-mode`).
    pub fn set_controllers_mode(&mut self, play: bool, mode: omarchyvr_config::ctrl_inhand::CtrlMode) {
        if play {
            self.toggles.ctrl_play = mode;
        } else {
            self.toggles.ctrl_work = mode;
        }
        self.save_state();
        let running_side = self.live.started_as.as_deref().map(|k| k == "vr");
        let live = self.live.ctrl_inhand.is_some() && running_side == Some(play);
        let side = if play { "Play" } else { "Work" };
        if !live || self.demo {
            self.message = format!("Controllers in {side}: {} (from the next start)", mode.label().to_lowercase());
            return;
        }
        let path = self.paths.runtime_dir.join("pimax-ctrl-inhand-mode");
        self.task(None, move || {
            let r = omarchyvr_config::ctrl_inhand::request(&path, mode, std::time::Duration::from_millis(1000));
            Box::new(move |c: &mut AppCore| {
                c.message = match r {
                    Ok(()) => format!("Controllers in {side}: {} (now and from the next start)", mode.label().to_lowercase()),
                    Err(e) => format!("Controllers in {side}: saved for the next start; {e}"),
                }
            })
        });
    }

    /// "Screen numbers" (off / on demand / always): saved at once in the WayVR settings (the next desk start writes it
    /// into WayVR's conf.d, so restarts keep it; unsaved edits of the WayVR page stay unsaved) and sent to a running
    /// WayVR (`wayvrctl screen-numbers off|ondemand|always`).
    pub fn set_screen_numbers(&mut self, mode: wayvr::ScreenNumbers) {
        self.wayvr.settings.screen_numbers = mode;
        if self.live.alive("wayvr") {
            self.wayvr_ctl(Ctl::ScreenNumbers(mode));
        }
        let mut s = self.wayvr.on_disk.clone().unwrap_or_default();
        s.screen_numbers = mode;
        if self.demo {
            self.wayvr.on_disk = Some(s);
            return;
        }
        let path = self.wayvr_paths.settings.clone();
        self.task(None, move || {
            let r = wayvr::save_settings(&path, &s);
            Box::new(move |c: &mut AppCore| match r {
                Ok(()) => {
                    if let Some(d) = c.wayvr.on_disk.as_mut() {
                        d.screen_numbers = mode;
                    } else {
                        c.wayvr.on_disk = Some(s);
                    }
                    c.message = format!("Screen numbers {}: saved for every desk start", match mode {
                        wayvr::ScreenNumbers::Off => "off",
                        wayvr::ScreenNumbers::OnDemand => "on demand",
                        wayvr::ScreenNumbers::Always => "always",
                    });
                }
                Err(e) => c.message = format!("cannot save {}: {e}", path.display()),
            })
        });
    }

    /// The gaze dot switch: saved in `eye.conf` (read by the next desk start) and sent to a running WayVR.
    pub fn set_eye_dot(&mut self, on: bool) {
        self.eye_settings.dot = on;
        self.save_eye_settings();
        if self.live.alive("wayvr") {
            self.set_eye_laser(on);
        }
    }

    /// Developer: gaze may target windows for voice (off by default; takes effect at the next desk start).
    pub fn set_eye_targeting(&mut self, on: bool) {
        self.eye_settings.targeting = on;
        self.save_eye_settings();
    }

    fn save_eye_settings(&mut self) {
        if self.demo {
            return;
        }
        if let Err(e) = self.eye_settings.save(&omarchyvr_config::eye::default_path()) {
            self.message = format!("cannot save the eye settings: {e}");
        }
    }

    /// The eye calibration in use: the user's own file once it exists (setting `auto`), else the built-in test one.
    pub fn eye_cal_source(&self) -> omarchyvr_config::eye::CalSource {
        self.eye_settings.calibration.source(self.facts.eye_calibration)
    }

    /// The eye calibration choice: saved in `eye.conf` (read by the next session start).
    pub fn set_eye_calibration(&mut self, mode: omarchyvr_config::eye::CalMode) {
        self.eye_settings.calibration = mode;
        self.save_eye_settings();
    }

    /// The eye tracking status in plain words.
    pub fn eye_status(&self) -> (String, bool) {
        let usable = self.facts.eye_calibration || self.eye_cal_source() == omarchyvr_config::eye::CalSource::Builtin;
        eye_status_text(self.toggles.eye, usable, self.facts.caps.eye || self.facts.caps.no_eye, self.live.service.as_ref().map(|s| s.eye))
    }

    /// Which start list and which calibration the eyes use, in plain words (the Eyes card).
    pub fn eye_sources_text(&self) -> String {
        eye_sources_text(pimax_eye::frames::list().1, self.eye_cal_source(), self.facts.eye_calibration)
    }

    /// The eye laser of the running session (`wayvrctl gaze-laser on|off`).
    pub fn set_eye_laser(&mut self, on: bool) {
        self.eye_laser = Some(on);
        self.wayvr_ctl(Ctl::GazeLaser(on));
    }

    /// Back up (or reset: back up and remove) the saved screen layout. A reset needs WayVR stopped: it rewrites the
    /// file when it exits.
    pub fn wayvr_layout(&mut self, reset: bool) {
        if reset && self.live.alive("wayvr") {
            self.message = "Stop desk mode first: WayVR saves its layout when it exits".into();
            return;
        }
        if self.demo {
            self.message = format!("demo: would {} the saved layout", if reset { "reset" } else { "back up" });
            return;
        }
        let w = self.wayvr_paths.clone();
        let stamp = omarchyvr_config::store::stamp(self.machine.now_unix());
        self.task(None, move || {
            let r = if reset { wayvr::reset_layout(&w, &stamp) } else { wayvr::backup_layout(&w, &stamp) };
            Box::new(move |c: &mut AppCore| {
                c.message = match r {
                    Ok(b) if reset => format!("Layout reset; backup in {}", b.display()),
                    Ok(b) => format!("Layout backed up to {}", b.display()),
                    Err(e) => format!("layout: {e}"),
                };
                c.wayvr.at = None;
            })
        });
    }

    /// Keep the Auto mode row fresh while it is open (status every 2 s, in the background; demo data in demo mode).
    pub fn want_auto(&mut self) {
        if self.demo {
            if self.auto.status.is_none() {
                self.auto.status = Some(Ok(crate::auto::demo_status()));
            }
            return;
        }
        if self.auto.at.is_some_and(|t| t.elapsed() < Duration::from_secs(2)) {
            return;
        }
        self.auto.at = Some(Instant::now());
        let sock = self.auto_socket.clone();
        self.task(Some("auto"), move || {
            let r = crate::auto::request(&sock, &crate::auto::AutoCmd::Status);
            Box::new(move |c: &mut AppCore| c.auto.status = Some(r))
        });
    }

    /// Send a command to the auto mode daemon (the reply is its new status).
    pub fn auto_cmd(&mut self, cmd: crate::auto::AutoCmd) {
        if self.demo {
            if let Some(Ok(s)) = self.auto.status.as_mut() {
                match &cmd {
                    crate::auto::AutoCmd::Pause => s.paused = true,
                    crate::auto::AutoCmd::Resume => s.paused = false,
                    crate::auto::AutoCmd::SetMode { mode, runtime, preset } => {
                        if let Some(m) = mode {
                            s.mode = m.clone();
                        }
                        if let Some(r) = runtime {
                            s.game_runtime = r.clone();
                        }
                        if let Some(p) = preset {
                            s.preset = p.clone();
                        }
                    }
                    _ => {}
                }
            }
            self.auto.last = format!("demo: would send {}", cmd.json());
            return;
        }
        let sock = self.auto_socket.clone();
        let label = cmd.label();
        self.task(Some("auto-cmd"), move || {
            let r = crate::auto::request(&sock, &cmd);
            log::info!("auto mode {label}: {}", match &r { Ok(s) => s.state.clone(), Err(e) => e.clone() });
            Box::new(move |c: &mut AppCore| {
                c.auto.last = match &r {
                    Ok(_) => format!("{label}: done"),
                    Err(e) => format!("{label}: {e}"),
                };
                if r.is_ok() {
                    c.auto.status = Some(r);
                }
            })
        });
    }

    /// Pair a controller (`left` / `right`): one StartPair through the auto mode daemon (approved 2026-10-14,
    /// LESSONS row 11), or `omarchyvr auto pair <side>` when the daemon does not run. Demo mode only simulates it.
    pub fn pair(&mut self, side: &'static str) {
        use crate::auto::PairState;
        if self.auto.pair.as_ref().is_some_and(|p| p.2 == PairState::Waiting) {
            self.message = "A pairing is already running".into();
            return;
        }
        if let Some(s) = self.pair_wait(side) {
            // at most one StartPair per hand every 30 s (docs/ARCHITECTURE.md 4)
            self.message = format!("The {side} controller can be paired again in {s} s.");
            return;
        }
        self.auto.pair_at[usize::from(side == "right")] = Some(Instant::now());
        self.auto.pair = Some((side, Instant::now(), PairState::Waiting, crate::auto::PAIR_HINT.into()));
        if self.demo {
            return;
        }
        let daemon = matches!(self.auto.status, Some(Ok(_)));
        if !daemon {
            // without the daemon: the launcher's client does the same (exit 0 = paired)
            let c = Cmd::with_args(self.launcher_bin().display().to_string(), vec!["auto".into(), "pair".into(), side.into()]);
            self.spawn(JobSpec { kind: JobKind::Other(format!("pair:{side}")), label: format!("Pair the {side} controller"), steps: vec![Step::Run(c)] });
            return;
        }
        let sock = self.auto_socket.clone();
        self.task(Some("pair"), move || {
            let r = crate::auto::pair(&sock, side);
            log::info!("pairing {side}: {r:?}");
            Box::new(move |c: &mut AppCore| {
                let (st, detail) = match r {
                    Ok(x) => x,
                    Err(e) => (PairState::Failed, e),
                };
                if let Some(p) = c.auto.pair.as_mut() {
                    p.2 = st;
                    p.3 = detail;
                }
            })
        });
    }

    /// Unpair a controller (`left` / `right`): `omarchyvr controllers unpair <side>` in the background; the plain-words
    /// result goes to the message line. Refused while a pairing runs. Demo mode only simulates it.
    pub fn unpair(&mut self, side: &'static str) {
        use crate::auto::PairState;
        if self.auto.pair.as_ref().is_some_and(|p| p.2 == PairState::Waiting) {
            self.message = "A pairing is running; wait until it ends before unpairing.".into();
            return;
        }
        if let Some(who) = crate::auto::vr_session_owner().filter(|_| !self.demo) {
            self.message = format!("{who} is running and owns the headset: stop VR first, then unpair.");
            return;
        }
        if self.demo {
            self.message = format!("The {side} controller was unpaired (demo).");
            return;
        }
        let bin = self.launcher_bin();
        self.task(Some("unpair"), move || {
            let r = std::process::Command::new(&bin).args(["controllers", "unpair", side]).output();
            let msg = match r {
                Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
                Ok(o) => format!("Could not unpair the {side} controller: {}", String::from_utf8_lossy(&o.stderr).trim().trim_start_matches("omarchyvr: ")),
                Err(e) => format!("Could not unpair the {side} controller: {e}"),
            };
            log::info!("unpair {side}: {msg}");
            Box::new(move |c: &mut AppCore| {
                c.message = msg;
                if let Some(p) = c.auto.pair.as_mut().filter(|p| p.0 == side) {
                    p.2 = PairState::Failed;
                    p.3 = "Unpaired".into();
                }
            })
        });
    }

    /// "Test vibration" on a controller card: Pimax Play's test pulse (25 %, 300 ms) through `omarchyvr controllers buzz`,
    /// which goes through the running VR session (Monado or SteamVR) when there is one, else straight to the headset.
    /// The result shows on the message line.
    pub fn buzz(&mut self, side: &'static str) {
        if self.demo {
            self.message = format!("The {side} controller vibrated (demo).");
            return;
        }
        let bin = self.launcher_bin();
        self.task(Some("buzz"), move || {
            let r = std::process::Command::new(&bin).args(["controllers", "buzz", side]).output();
            let msg = match r {
                Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
                Ok(o) => format!(
                    "Could not vibrate the {side} controller: {}",
                    String::from_utf8_lossy(&o.stderr).trim().trim_start_matches("omarchyvr: ")
                ),
                Err(e) => format!("Could not vibrate the {side} controller: {e}"),
            };
            log::info!("buzz {side}: {msg}");
            Box::new(move |c: &mut AppCore| c.message = msg)
        });
    }

    /// Keep the Room row fresh while it is open (every 3 s, in the background).
    pub fn want_room(&mut self) {
        if self.room.at.is_some_and(|t| t.elapsed() < Duration::from_secs(3)) {
            return;
        }
        self.room.at = Some(Instant::now());
        let (m, r) = (self.machine.clone(), self.room_paths.clone());
        self.task(Some("room"), move || {
            let st = room::collect(m.as_ref(), &r);
            Box::new(move |c: &mut AppCore| {
                c.room.state = st;
                c.room.loaded = true;
            })
        });
    }

    /// Why a room action cannot run now (empty: it can).
    pub fn room_blockers(&self, a: RoomAction) -> Vec<String> {
        let st = &self.room.state;
        let (bin_ok, needs_map) = match a {
            RoomAction::MapRoom => (st.tools.0, None),
            RoomAction::Recognise => (st.tools.0, Some(self.room.map_name.as_str())),
            RoomAction::ViewMap => {
                let mut v = Vec::new();
                if !st.tools.0 && self.room.loaded {
                    v.push("The tool is not built (cargo build --release -p omarchyvr-roommap -p omarchyvr-anchor).".to_string());
                }
                if !st.maps.iter().any(|m| *m == self.room.map_name) {
                    v.push(format!("No room map named \"{}\" yet: map the room first.", self.room.map_name));
                }
                return v;
            }
            RoomAction::TagCalibrate | RoomAction::TagApply | RoomAction::Clear => (st.tools.1, None),
        };
        let mut v = Vec::new();
        if !bin_ok && self.room.loaded {
            v.push("The tool is not built (cargo build --release -p omarchyvr-roommap -p omarchyvr-anchor).".to_string());
        }
        if a == RoomAction::Clear {
            if st.offset_pid.is_none() {
                v.push("No room offset is active.".into());
            }
            return v;
        }
        if matches!(a, RoomAction::MapRoom | RoomAction::Recognise) && !room::valid_name(&self.room.map_name) {
            v.push("Map names are letters, digits, - and _.".into());
        }
        v.extend(room::blockers(&self.live, st, needs_map));
        if let (Some(s), RoomAction::Recognise | RoomAction::TagApply) = (&self.live.service, a) {
            let who = if a == RoomAction::Recognise { Writer::RoomMap } else { Writer::Tag };
            if let Err(e) = room::may_write(st, s.pid, who) {
                v.push(e);
            }
        }
        v
    }

    /// Run a room action (a job: its output goes to the Output panel).
    pub fn room_action(&mut self, a: RoomAction) {
        let b = self.room_blockers(a);
        if !b.is_empty() {
            self.message = b.join(" ");
            return;
        }
        let r = self.room_paths.clone();
        let name = self.room.map_name.clone();
        let pid = self.live.service.as_ref().map(|s| s.pid).unwrap_or(0);
        let (label, cmd, kind) = match a {
            RoomAction::MapRoom => (format!("Map the room \"{name}\""), room::build_cmd(&r, &name, self.room.seconds), "room:build".to_string()),
            RoomAction::Recognise => (format!("Recognise the room \"{name}\""), room::relocalize_cmd(&r, &name), format!("room:roommap:{pid}")),
            RoomAction::ViewMap => (format!("View the room map \"{name}\""), room::view_cmd(&r, &name), "room:view".to_string()),
            RoomAction::TagCalibrate => ("Calibrate the AprilTag".to_string(), room::anchor_cmd(&r, "calibrate"), "room:calibrate".to_string()),
            RoomAction::TagApply => ("Apply the AprilTag".to_string(), room::anchor_cmd(&r, "apply"), format!("room:anchor:{pid}")),
            RoomAction::Clear => ("Clear the room offset".to_string(), room::anchor_cmd(&r, "clear"), "room:clear".to_string()),
        };
        let mut steps = Vec::new();
        if matches!(a, RoomAction::Recognise | RoomAction::TagApply) {
            let who = if a == RoomAction::Recognise { Writer::RoomMap } else { Writer::Tag };
            let rp = r.clone();
            // checked again right before the tool writes the offset
            let g: job::GuardFn = Arc::new(move |m: &dyn Machine, log: &mut dyn FnMut(String)| {
                let st = room::collect(m, &rp);
                log(format!("offset of Monado pid {pid}: {:?}, recorded writer {:?}", st.offset_pid, st.owner.map(|(w, p)| (w.name(), p))));
                room::may_write(&st, pid, who)
            });
            steps.push(Step::Guard("only one room offset writer per session", g));
        }
        if a == RoomAction::MapRoom {
            steps.push(Step::Note(format!(
                "Walk slowly through the room for {} s: look at walls, shelves and the desk (not only one spot), turn around, and come back past where you started.",
                self.room.seconds
            )));
        }
        steps.push(Step::Run(cmd));
        self.spawn(JobSpec { kind: JobKind::Other(kind), label, steps });
    }

    fn room_job_done(&mut self, kind: &str, ok: bool) {
        self.room.at = None;
        if !ok || self.demo {
            return;
        }
        let owner = self.room_paths.owner.clone();
        let rec = match kind.split(':').collect::<Vec<_>>().as_slice() {
            ["room", "roommap", pid] => Some(pid.parse::<u32>().ok().map(|p| (Writer::RoomMap, p))),
            ["room", "anchor", pid] => Some(pid.parse::<u32>().ok().map(|p| (Writer::Tag, p))),
            ["room", "clear"] => Some(None),
            _ => None,
        };
        if let Some(who) = rec {
            self.task(None, move || {
                let r = room::write_owner(&owner, who);
                Box::new(move |c: &mut AppCore| {
                    if let Err(e) = r {
                        c.message = format!("cannot record the offset writer: {e}");
                    }
                    c.room.at = None;
                })
            });
        }
    }

    /// Start SteamVR through Steam (steam://rungameid/250820).
    pub fn launch_steamvr(&mut self) {
        if self.demo {
            self.message = "demo: would start SteamVR".into();
            return;
        }
        self.message = "Starting SteamVR through Steam...".into();
        self.task(None, move || {
            let r = crate::cmd::allow_start("xdg-open").and_then(|()| {
                std::process::Command::new("xdg-open")
                    .arg("steam://rungameid/250820")
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .map_err(|e| format!("Could not start SteamVR: {e}"))
                    .map(|mut ch| {
                        std::thread::spawn(move || {
                            let _ = ch.wait();
                        });
                    })
            });
            Box::new(move |c: &mut AppCore| {
                if let Err(e) = r {
                    c.message = e;
                }
            })
        });
    }

    /// Open a folder in the file manager (`xdg-open`, in the background).
    pub fn open_folder(&mut self, dir: &Path) {
        if self.demo {
            self.message = format!("demo: would open {}", dir.display());
            return;
        }
        let dir = dir.to_path_buf();
        self.task(None, move || {
            let r = crate::cmd::allow_start("xdg-open").and_then(|()| {
                std::process::Command::new("xdg-open")
                    .arg(&dir)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .map_err(|e| format!("xdg-open: {e}"))
                    .map(|mut ch| {
                        std::thread::spawn(move || {
                            let _ = ch.wait();
                        });
                    })
            });
            Box::new(move |c: &mut AppCore| {
                if let Err(e) = r {
                    c.message = e;
                }
            })
        });
    }
}

/// Logs the Logs page offers: (label, path), existing ones only.
pub fn log_sources(m: &dyn Machine, paths: &Paths) -> Vec<(String, PathBuf)> {
    let mut v: Vec<(String, PathBuf)> = Vec::new();
    for f in ["monado.log", "monado.log.prev", "passthrough.log", "wayvr.log", "wayvr.stdout", "broker.log", "ctrltrack.log", "voice.log", "voicecap.log", "voicehud.log"] {
        let p = paths.session_dir.join(f);
        if m.exists(&p) {
            v.push((format!("session: {f}"), p));
        }
    }
    // 6DoF test runs (LESSONS row 17)
    if let Some(last) = m.list(&paths.slam_recordings).into_iter().filter(|n| n.ends_with("-live")).max() {
        let p = paths.slam_recordings.join(&last).join("monado.log");
        if m.exists(&p) {
            v.push((format!("6DoF run {last}: monado.log"), p));
        }
    }
    let tmp = Path::new("/tmp");
    let newest = m
        .list(tmp)
        .into_iter()
        .filter(|n| n.starts_with("pimax-slamview."))
        .max_by_key(|n| m.meta(&tmp.join(n)).map(|x| x.mtime).unwrap_or(0));
    if let Some(d) = newest {
        for f in ["broker.log", "slam.log", "passthrough.log"] {
            let p = tmp.join(&d).join(f);
            if m.exists(&p) {
                v.push((format!("walk test {d}: {f}"), p));
            }
        }
    }
    // the app's own log
    let own = crate::applog::log_file(&paths.state_dir);
    if m.exists(&own) {
        v.push(("app: omarchyvr-app.log".into(), own));
    }
    v
}

/// The last `n` lines of a log (reads at most 512 KiB from the end).
pub fn tail(m: &dyn Machine, path: &Path, n: usize) -> Vec<String> {
    let len = m.meta(path).map(|x| x.len).unwrap_or(0);
    let from = len.saturating_sub(512 * 1024);
    let Some(b) = m.read_range(path, from, 512 * 1024) else { return Vec::new() };
    let text = String::from_utf8_lossy(&b);
    let lines: Vec<&str> = text.lines().collect();
    let skip_first = from > 0;
    let start = lines.len().saturating_sub(n).max(usize::from(skip_first));
    lines[start.min(lines.len())..].iter().map(|s| s.to_string()).collect()
}

/// INVENTORY and LESSONS from a docs directory.
pub fn load_docs(dir: &Path) -> Docs {
    let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap_or_default();
    let inv = read("INVENTORY.md");
    let les = read("LESSONS.md");
    Docs {
        history: docs::history(&inv),
        open_issues: docs::open_issues(&inv),
        safety: docs::safety_rules(&inv),
        lessons: docs::lessons(&les),
        source: dir.to_path_buf(),
    }
}

/// The Desk page state from the running job and the live state.
pub fn desk_phase(job: Option<&JobKind>, live: &Live) -> DeskPhase {
    match job {
        Some(JobKind::DeskStart) => DeskPhase::Starting,
        Some(JobKind::Stop) => DeskPhase::Stopping,
        Some(JobKind::ToolSession(_)) => DeskPhase::Starting,
        Some(_) if live.service.is_none() => DeskPhase::Busy,
        _ => {
            // SteamVR started by Play (Games run with: SteamVR) is a running game session: Play stops it
            if live.service.is_some() || live.steamvr.is_some() {
                DeskPhase::Running
            } else if !live.foreign_monado.is_empty() {
                DeskPhase::Foreign
            } else if live.session_helpers_alive() {
                DeskPhase::Stopping
            } else {
                DeskPhase::Idle
            }
        }
    }
}

/// SIGINT to `pid` if it is still the same process (name and start time); never to monado-service (that goes
/// through `omarchyvr vr stop`), never to pid 0/1, the app itself or a pid that would mean a process group.
pub fn interrupt_pid(m: &dyn Machine, pid: u32, comm: &str, start: Option<u64>, dry: bool) -> Result<String, String> {
    if comm == procs::MONADO_COMM {
        return Err("monado-service is only stopped with omarchyvr vr stop --grace 30".into());
    }
    if pid <= 1 || pid == std::process::id() || pid > i32::MAX as u32 {
        return Err(format!("pid {pid} is not signalled by the app"));
    }
    match procs::comm(m, pid) {
        Some(c) if c == procs::MONADO_COMM => return Err("monado-service is only stopped with omarchyvr vr stop --grace 30".into()),
        Some(c) if c == comm && procs::is_alive(m, pid) => {}
        Some(c) => return Err(format!("pid {pid} is now {c}, not {comm}: not touching it")),
        None => return Ok(format!("pid {pid} has already exited")),
    }
    if start.is_some() && procs::start_ticks(m, pid) != start {
        return Err(format!("pid {pid} is another process now (it started later): not touching it"));
    }
    if dry {
        return Ok(format!("demo: would send SIGINT to pid {pid} ({comm})"));
    }
    // SAFETY: kill(2) with a positive pid (one process, never a group) and SIGINT.
    let r = unsafe { libc::kill(pid as libc::pid_t, libc::SIGINT) };
    if r == 0 {
        Ok(format!("SIGINT sent to pid {pid} ({comm})"))
    } else {
        Err(format!("SIGINT to pid {pid} failed: {}", std::io::Error::last_os_error()))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_old_local_ai_page_name_opens_the_ai_page() {
        assert_eq!(Page::from_name("local-ai"), Some(Page::Ai));
        assert_eq!(Page::from_name("ai"), Some(Page::Ai));
        assert!(Page::ALL.iter().all(|p| Page::from_name(p.name()) == Some(*p)));
    }

    use super::*;
    use crate::facts::SessionProc;

    fn store(tag: &str) -> (Store, PathBuf) {
        let d = std::env::temp_dir().join(format!("omarchyvr-gui-core-app-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        (Store::new(d.join("cfg"), d.join("cache")), d)
    }

    #[test]
    fn desk_start_job_waits_rechecks_and_needs_consent() {
        let (s, d) = store("start");
        let mut a = AppCore::demo(false, s);
        // the demo PC has WayVR Xid 31 faults: no start without consent
        assert!(a.desk_start_job(false).unwrap_err().contains("needs your OK"));
        let j = a.desk_start_job(true).unwrap();
        assert_eq!(j.kind, JobKind::DeskStart);
        assert!(matches!(j.steps[0], Step::WaitNoMonado(t) if t == Duration::from_secs(45)), "row 1 first");
        assert!(matches!(j.steps[1], Step::Guard("camera and eye chip still free", _)));
        assert!(matches!(j.steps[2], Step::Guard("still only WayVR Xid 31", _)), "--force re-checked right before");
        match &j.steps[3] {
            Step::Run(c) => {
                assert_eq!(c.display(), "OXR_DEBUG_IPD_MM=65.9 /home/u/.local/bin/omarchyvr desk start --fps 90 --eye --6dof --force");
                assert_eq!(crate::safety::check(c), Ok(()));
            }
            s => panic!("{s:?}"),
        }
        // the guard passes on the idle machine and fails when the camera is taken
        if let Step::Guard(_, g) = &j.steps[1] {
            let mut lines = Vec::new();
            assert_eq!(g(a.machine.as_ref(), &mut |l| lines.push(l)), Ok(()));
            let (mut m, _) = crate::demo::idle_machine();
            m.process(4242, "pimax-cam", "pimax-cam broker", &[], 1, &["/dev/video1"]);
            assert!(g(&m, &mut |_| {}).unwrap_err().contains("camera busy"));
        }
        // in demo mode nothing starts
        a.consent_force = true;
        a.start_desk();
        assert!(a.job.is_none() && a.message.starts_with("demo mode"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn starts_run_in_their_own_user_scope_when_systemd_is_there() {
        let (s, d) = store("scope");
        let mut a = AppCore::demo(false, s);
        let plain = Cmd::new("/home/u/.local/bin/omarchyvr", &["desk", "start"]);
        assert_eq!(a.own_scope(plain.clone(), "desk"), plain, "no systemd-run on the demo PC: unchanged");
        let mut m = crate::demo::idle_machine().0;
        m.file("/usr/bin/systemd-run", "elf");
        m.file("/run/user/1000/systemd/private", "");
        m.set_env("PATH", "/usr/local/bin:/usr/bin");
        a.machine = std::sync::Arc::new(m);
        a.refresh_now(false);
        assert_eq!(a.facts.systemd_run.as_deref(), Some(Path::new("/usr/bin/systemd-run")), "found by the background check");
        assert!(a.facts.user_manager);
        let w = a.own_scope(plain.env("OXR_DEBUG_IPD_MM", "65.9"), "desk");
        assert_eq!(w.program, "/usr/bin/systemd-run");
        assert_eq!(w.args[..2], ["--user".to_string(), "--scope".to_string()]);
        assert!(w.args.ends_with(&["--".to_string(), "/home/u/.local/bin/omarchyvr".to_string(), "desk".to_string(), "start".to_string()]));
        assert!(w.env.contains(&("OXR_DEBUG_IPD_MM".to_string(), "65.9".to_string())), "the scope keeps the environment");
        assert_eq!(crate::safety::check(&w), Ok(()));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn stop_uses_the_launcher_and_restart_without_eye_is_ordered() {
        let (s, d) = store("stop");
        let mut a = AppCore::demo(true, s);
        assert_eq!(a.phase(), DeskPhase::Running);
        // the launcher exits 2 for any Xid of this boot: the exit code is only reported, then Monado must be gone
        let stop = a.stop_job();
        match &stop.steps[0] {
            Step::RunAllowFail(c) => assert_eq!(c.display(), "/home/u/.local/bin/omarchyvr desk stop --grace 30"),
            s => panic!("{s:?}"),
        }
        assert!(matches!(stop.steps[1], Step::WaitNoMonado(_)));
        // restart without eye: gated like a start; the demo PC needs the consent over its WayVR Xid 31 faults
        assert!(a.restart_without_eye_job().unwrap_err().contains("Start anyway"));
        a.consent_force = true;
        let j = a.restart_without_eye_job().unwrap();
        let kinds: Vec<String> = j.steps.iter().map(|s| format!("{s:?}")).collect();
        assert!(kinds[0].starts_with("RunAllowFail") && kinds[0].contains("desk stop --grace 30"));
        assert!(kinds[1].starts_with("WaitNoMonado"));
        assert!(kinds[2].starts_with("Guard(camera still free)"));
        assert!(kinds[3].starts_with("Guard(still only WayVR Xid 31)"));
        assert!(kinds[4].contains("desk start --fps 90 --6dof --force") && !kinds[4].contains("--eye"));
        // the preflight while running says: stop first
        assert!(matches!(a.preflight().gate, Gate::Blocked(_)));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn pairing_in_demo_is_simulated_and_the_cli_fallback_is_safe() {
        let (s, d) = store("pair");
        let mut a = AppCore::demo(true, s);
        a.pair("left");
        assert!(matches!(a.auto.pair, Some(("left", _, crate::auto::PairState::Waiting, _))));
        assert!(a.job.is_none(), "nothing sent in demo mode");
        a.pair("right");
        assert_eq!(a.message, "A pairing is already running");
        let c = Cmd::with_args(a.launcher_bin().display().to_string(), vec!["auto".into(), "pair".into(), "left".into()]);
        assert_eq!(crate::safety::check(&c), Ok(()));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn phases() {
        let mut live = Live::default();
        assert_eq!(desk_phase(None, &live), DeskPhase::Idle);
        assert_eq!(desk_phase(Some(&JobKind::DeskStart), &live), DeskPhase::Starting);
        assert_eq!(desk_phase(Some(&JobKind::Fix("x".into())), &live), DeskPhase::Busy);
        live.foreign_monado = vec![5];
        assert_eq!(desk_phase(None, &live), DeskPhase::Foreign);
        live.foreign_monado.clear();
        live.session.push(SessionProc { name: "wayvr", pid: Some(3), alive: true, age: None });
        assert_eq!(desk_phase(None, &live), DeskPhase::Stopping, "clients without Monado: leftovers");
        assert_eq!(desk_phase(Some(&JobKind::Stop), &live), DeskPhase::Stopping);
    }

    #[test]
    fn interrupt_only_by_pid_with_the_same_name() {
        let (mut m, _) = crate::demo::idle_machine();
        m.process(4243, "pimax-eye", "pimax-eye watch", &[], 77, &[]);
        let start = procs::start_ticks(&m, 4243);
        assert_eq!(start, Some(77));
        assert!(interrupt_pid(&m, 4243, "pimax-eye", start, true).unwrap().contains("would send SIGINT"));
        assert!(interrupt_pid(&m, 4243, "wayvr", start, true).unwrap_err().contains("not touching"));
        assert!(interrupt_pid(&m, 4243, "pimax-eye", Some(5), true).unwrap_err().contains("another process"), "pid reuse");
        assert!(interrupt_pid(&m, 9999, "x", None, true).unwrap().contains("already exited"));
        assert!(interrupt_pid(&m, 1, "systemd", None, true).is_err());
        assert!(interrupt_pid(&m, std::process::id(), "x", None, true).is_err());
        assert!(interrupt_pid(&m, u32::MAX, "x", None, true).is_err());
        m.process(4244, "monado-service", "/x/monado-service", &[], 1, &[]);
        assert!(interrupt_pid(&m, 4244, "monado-service", None, true).is_err());
        assert!(interrupt_pid(&m, 4244, "renamed", None, true).is_err(), "the real name is checked too");
    }

    #[test]
    fn fixes_respect_idle_and_follow_up_with_setcap() {
        let (s, d) = store("fix");
        let mut a = AppCore::demo(true, s);
        a.rebuild_desk();
        assert!(a.message.contains("row 8"), "{}", a.message);
        let (s2, d2) = store("fix2");
        let mut b = AppCore::demo(false, s2);
        b.rebuild_desk();
        assert!(b.message.starts_with("demo mode: would run Fix"), "{}", b.message);
        // the follow-up after a successful rebuild is the setcap prompt
        b.follow_up = Some(FollowUp::Setcap(b.paths.desk_build().service()));
        b.job_finished(JobKind::Fix("rebuild-desk".into()), "Fix", Ok(()));
        match &b.pending {
            Some(Pending::Root { fix, .. }) => assert!(fix.lines[0].starts_with("sudo setcap cap_sys_nice+ep /home/u/.local/lib/omarchyvr/monado")),
            p => panic!("{p:?}"),
        }
        // root fixes: only after confirmation; the slam build's setcap is runnable
        b.pending = None;
        b.ask_root("setcap-slam");
        assert!(matches!(b.pending, Some(Pending::Root { .. })));
        b.facts.polkit_agent = true;
        b.confirm_root();
        assert!(b.pending.is_none() && b.message.starts_with("demo mode: would run Root fix"), "{}", b.message);
        // without a polkit agent the same pkexec command goes to a terminal
        b.facts.polkit_agent = false;
        b.ask_root("setcap-slam");
        b.confirm_root();
        assert!(b.message.starts_with("demo mode: would open a terminal for: /usr/bin/pkexec /bin/sh -c"), "{}", b.message);
        let _ = std::fs::remove_dir_all(d);
        let _ = std::fs::remove_dir_all(d2);
    }

    #[test]
    fn logs_and_docs() {
        let (s, d) = store("logs");
        let mut a = AppCore::demo(true, s);
        let src = log_sources(a.machine.as_ref(), &a.paths);
        assert!(src.iter().any(|(l, _)| l == "session: monado.log"));
        let t = tail(a.machine.as_ref(), &a.paths.session_dir.join("monado.log"), 3);
        assert_eq!(t.len(), 3);
        assert!(t[2].contains("Pimax eye: state"));
        // the page asks; the (inline) worker answers
        a.want_logs(0, true);
        assert!(a.logs.listed && a.logs.sources == src);
        assert_eq!(a.logs.tail.as_ref().map(|(p, _)| p.clone()), Some(src[0].1.clone()));
        let docs = a.docs().expect("inline: loaded right away");
        assert!(docs.history.as_ref().is_some_and(|h| h.rows.len() > 10));
        assert!(docs.lessons.len() >= 5);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn tool_session_flow_in_demo() {
        let (s, d) = store("tool");
        let mut a = AppCore::demo(false, s);
        a.run_tool(ToolId::LensTest);
        assert!(a.message.contains("Xid 31"), "consent needed: {}", a.message);
        a.tool_opts.consent_force = true;
        a.run_tool(ToolId::LensTest);
        assert!(a.message.starts_with("demo mode: would run Lens test"), "{}", a.message);
        assert!(a.session_tool.is_none(), "nothing started in demo mode");
        a.run_tool(ToolId::EyeView);
        assert!(a.message.contains("opened in a terminal") || a.message.contains("no terminal"), "{}", a.message);
        let _ = std::fs::remove_dir_all(d);
    }
}

/// The microphone test job's kind.
pub const MIC_TEST_KIND: &str = "sound mic-test";

/// The speaker test job's kind.
pub const SPEAKER_TEST_KIND: &str = "sound speaker-test";

/// A test's output in plain words (its last line starting with `prefix`).
pub fn test_result_from(lines: &[String], prefix: &str, ok: bool) -> String {
    match lines.iter().rev().find_map(|l| l.split_once(prefix).map(|(_, r)| r.trim().to_string())) {
        Some(r) => r,
        None if ok => "The test ran but printed no result (see the output below).".into(),
        None => "The test did not run (see the output below).".into(),
    }
}

/// The microphone test's output in plain words (its "Microphone test:" line).
pub fn mic_result_from(lines: &[String], ok: bool) -> String {
    test_result_from(lines, "Microphone test:", ok)
}

#[cfg(test)]
mod mic_test_tests {
    #[test]
    fn mic_result_in_plain_words() {
        let l = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let r = super::mic_result_from(&l(&["Recording ...", "Microphone test: headset microphone, level -30 dBFS (peak -8 dBFS, 3.0 s, 2 ch): good", "Playing it back on x ..."]), true);
        assert_eq!(r, "headset microphone, level -30 dBFS (peak -8 dBFS, 3.0 s, 2 ch): good");
        assert!(super::mic_result_from(&l(&[]), false).contains("did not run"));
        let s = super::test_result_from(&l(&["Speaker test: no headset speaker: it exists only while VR runs"]), "Speaker test:", false);
        assert!(s.starts_with("no headset speaker"));
    }
}

/// The start list and the calibration in use, in plain words.
pub fn eye_sources_text(list: pimax_eye::frames::ListKind, cal: omarchyvr_config::eye::CalSource, own_exists: bool) -> String {
    use omarchyvr_config::eye::CalSource;
    use pimax_eye::frames::ListKind;
    let l = match list {
        ListKind::Base => "Start list: the base list (no sign-in step, tested on one headset).",
        ListKind::Full => "Start list: the full list of this build.",
    };
    let c = match (cal, own_exists) {
        (CalSource::Builtin, _) => "Calibration: the built-in test calibration. It was made on someone else's eyes, so the aim is rough until you calibrate your own.",
        (CalSource::Own, true) => "Calibration: your own.",
        (CalSource::Own, false) => "Calibration: your own, but none is saved yet, so eye tracking does not run. Calibrate, or switch to the built-in one.",
    };
    format!("{l} {c}")
}

/// Plain-words eye tracking status: (text, ok). `running`: Some(eye flag of the running Monado).
pub fn eye_status_text(switch_on: bool, calibrated: bool, supported: bool, running: Option<bool>) -> (String, bool) {
    if !switch_on {
        return ("Off".into(), true);
    }
    if !supported {
        return ("Not available: this build of the launcher has no eye tracking".into(), false);
    }
    if !calibrated {
        return ("Not calibrated, so it does not run (no calibration saved)".into(), false);
    }
    match running {
        Some(true) => ("Running (visual only)".into(), true),
        Some(false) => ("Not running: the eye tracker did not start. VR works without it".into(), false),
        None => ("On: starts with the next session".into(), true),
    }
}

#[cfg(test)]
mod eye_status_tests {
    use super::eye_status_text as t;

    #[test]
    fn sources_text_names_the_list_and_whose_eyes() {
        use omarchyvr_config::eye::CalSource;
        use pimax_eye::frames::ListKind;
        let b = super::eye_sources_text(ListKind::Base, CalSource::Builtin, false);
        assert!(b.contains("base list") && b.contains("someone else's eyes") && b.contains("rough"));
        let o = super::eye_sources_text(ListKind::Full, CalSource::Own, true);
        assert!(o.contains("full list") && o.contains("your own") && !o.contains("someone else"));
        assert!(super::eye_sources_text(ListKind::Base, CalSource::Own, false).contains("does not run"));
    }

    #[test]
    fn plain_words() {
        assert_eq!(t(false, true, true, Some(true)).0, "Off");
        assert!(t(true, false, true, None).0.starts_with("Not calibrated"));
        assert!(!t(true, false, true, None).1);
        assert!(t(true, true, false, None).0.starts_with("Not available"));
        assert_eq!(t(true, true, true, Some(true)).0, "Running (visual only)");
        assert!(t(true, true, true, Some(false)).0.contains("VR works without it"));
        assert!(t(true, true, true, None).0.starts_with("On"));
    }
}
