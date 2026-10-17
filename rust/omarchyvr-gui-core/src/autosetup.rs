//! The automatic first-run setup: "Setting up your PC" runs by itself when the app opens.
//!
//! Four stages, no clicks: (1) the read-only checks, (2) the fixes that need no password, (3) the root job with ONE
//! pkexec prompt (its steps shown as a live list, parsed from the job's own output), (4) the checks again. Then
//! one of: Ready, "Restart now" (the job said `REBOOT=yes`, or a check waits for the restart), or a list of
//! problems, each with its plain reason and one Fix button (several root fixes are batched into "Fix all": one
//! password). After the restart the app starts again by itself (autostart) and continues with the post-restart
//! checks (patched module loaded, EDID override in place, headset display mode, realtime permission).
//!
//! [`AutoSetup`] is a pure state machine: events in ([`AutoSetup::checks_done`], [`AutoSetup::root_finished`], ...),
//! one [`Action`] out. It never runs anything, so the tests drive it with fake checks and a fake root job. The glue
//! that runs the actions (`impl AppCore` below) uses the existing jobs, the checklist and the pkexec path.
use crate::app::{AppCore, Pending};
use crate::checklist::{Fix, Item};
use crate::guistate::SetupStage;
use crate::headset_choice::{self, HeadsetPick};
use crate::install::{setup_fix, Install};
use omarchyvr_doctor::Level;
use std::path::Path;

/// State of one step of the live list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StepState {
    /// Not reached yet.
    #[default]
    Pending,
    /// Running now.
    Running,
    /// Done.
    Done,
    /// Not needed or not possible here (the reason is in the note); the setup goes on.
    Skipped,
    /// Failed.
    Failed,
}

impl StepState {
    /// One plain word.
    pub fn word(self) -> &'static str {
        match self {
            StepState::Pending => "waiting",
            StepState::Running => "running",
            StepState::Done => "done",
            StepState::Skipped => "skipped",
            StepState::Failed => "failed",
        }
    }
}

/// One step of the root job, as the live list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootStep {
    /// The job's own number.
    pub n: u32,
    /// Plain-words title.
    pub title: String,
    /// The job's original line.
    pub raw: String,
    /// State.
    pub state: StepState,
    /// A short note (why skipped, why failed).
    pub note: String,
}

/// The result of a check, as the setup sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    /// Fine.
    Ok,
    /// Wrong.
    Fail,
    /// Set up correctly, takes effect after the restart.
    WaitsForRestart,
}

/// What fixes a failed check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckFix {
    /// Nothing the app can do (the reason says what the person does).
    None,
    /// A fix as the user, no password.
    User,
    /// A fix as root (batched into one password prompt).
    Root {
        /// The fix needs a restart afterwards.
        reboot: bool,
    },
}

/// One check of the setup (a failed one is a problem).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// Checklist item id.
    pub id: String,
    /// Plain title.
    pub title: String,
    /// Result.
    pub state: CheckState,
    /// Plain reason (shown when it is not green).
    pub reason: String,
    /// The technical line (Show details).
    pub detail: String,
    /// The fix.
    pub fix: CheckFix,
}

/// How the root job ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootOutcome {
    /// Exit 0 (`reboot` = the job printed `REBOOT=yes`).
    Ok {
        /// The job asked for a restart.
        reboot: bool,
    },
    /// The password window was closed (pkexec exit 126).
    Cancelled,
    /// Wrong password / not allowed (pkexec exit 127).
    WrongPassword,
    /// The headset's display was not visible (EDID mode exit 3).
    NoHeadsetDisplay,
    /// Another failure; the text is the job's last message.
    Failed(String),
}

/// Why the password step stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// The password window was closed.
    Cancelled,
    /// The password was wrong (or this user may not do it).
    WrongPassword,
    /// The app could not even ask (no password window).
    NoDialog,
}

/// Where the run is.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Phase {
    /// Not started.
    #[default]
    Idle,
    /// First screen: "Which headset do you have?" (nothing has run, no password yet).
    ChooseHeadset,
    /// "Get the headset ready": plain steps for the chosen model, then "Start setup".
    GetReady,
    /// Stage 1.
    Looking,
    /// Stage 2.
    FixingUser,
    /// Stage 3.
    Password,
    /// Stage 4.
    Verifying,
    /// All green, the restart is the last step.
    NeedsRestart,
    /// Something is not green.
    Problems,
    /// Stage 3 stopped.
    PasswordStopped(Stop),
    /// The person pressed Cancel.
    Cancelled,
    /// All green.
    Ready,
}

/// Which run this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunKind {
    /// First start: the whole run.
    #[default]
    Fresh,
    /// "Run setup again" or "Try again": the whole run, the root job again.
    Again,
    /// The app started after the restart: only the post-restart checks.
    AfterReboot,
    /// The app started after an update by `./install.sh` (its `post-install-check` file): only the checks, once; a
    /// problem (for example the realtime permission of a changed Monado) shows at once with its one-password Fix.
    AfterUpdate,
}

impl RunKind {
    /// Only the checks run (the system setup ran before): after the restart, after an update.
    pub fn check_only(self) -> bool {
        matches!(self, RunKind::AfterReboot | RunKind::AfterUpdate)
    }
}

/// What the glue must do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Nothing.
    None,
    /// Collect the checks (the doctor and the checklist) and call [`AutoSetup::checks_done`].
    RunChecks,
    /// Run this user fix (checklist item id) and call [`AutoSetup::user_fix_done`].
    RunUserFix(String),
    /// Run the root job and call [`AutoSetup::root_finished`].
    RunRoot(RootPlan),
}

/// What the password step runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootPlan {
    /// The installer's root job (`omarchyvr-root.sh apply`).
    Setup,
    /// The root fixes of these checks, in one prompt.
    Fixes(Vec<String>),
}

/// The state machine of one run.
#[derive(Debug, Clone, Default)]
pub struct AutoSetup {
    /// Where the run is.
    pub phase: Phase,
    /// Which run.
    pub kind: RunKind,
    /// The four stages: look, fix without password, password, check again.
    pub stages: [StepState; 4],
    /// The root job's steps.
    pub root_steps: Vec<RootStep>,
    /// The root job's output so far (or the plan text before it runs): the "Show details" text.
    pub raw: String,
    /// Every check of the last collection.
    pub checks: Vec<Check>,
    /// What is not green.
    pub problems: Vec<Check>,
    /// A restart is needed (the job said so, or a check waits for it).
    pub reboot: bool,
    /// The person chose "Later".
    pub later: bool,
    /// The root job's failure text (the problems list shows it).
    pub root_error: Option<String>,
    /// The headset list: what is selected now (pre-selected from USB, or by the person).
    pub selected: HeadsetPick,
    /// "Found on USB: Pimax Dream Air (asleep)" when detection pre-selected something.
    pub found: Option<String>,
    /// Cancel was pressed: nothing new starts.
    pub cancel: bool,
    /// The root job ended with only the EDID step skipped (the headset's display was not visible then).
    pub edid_skipped: bool,
    queue: Vec<String>,
    from_problems: bool,
    root_job: bool,
    plan_steps: Vec<RootStep>,
}

/// The checklist ids the setup cares about (the rest of the checklist is the Setup page's business). The headset's parts
/// (`headset`: its control chip, `headset-camera`, `eye-chip`) are part of it since 2026-10-17: the setup is not
/// "ready" while one is missing (each shows with its one step and, when the headset needs waking or a power cycle, the
/// "Get the headset ready" guide).
pub const SETUP_IDS: [&str; 13] = [
    "headset", "headset-camera", "eye-chip", "udev", "udev-rule", "keepalive", "pacman-hooks", "modinfo", "merged-head-sync", "connector", "setcap-desk", "gpu", "lease",
];

/// The problems that the "Get the headset ready" guide helps with: `Some(false)` the control chip is missing (wake the
/// headset), `Some(true)` the cameras are missing (a power cycle), `None` nothing for the guide.
pub fn guide_for(problems: &[Check]) -> Option<bool> {
    if problems.iter().any(|p| p.id == "headset") {
        Some(false)
    } else if problems.iter().any(|p| p.id == "headset-camera") {
        Some(true)
    } else {
        None
    }
}

impl AutoSetup {
    /// A run that is over (Ready) or waiting for the restart.
    pub fn finished(&self) -> bool {
        matches!(self.phase, Phase::Ready | Phase::NeedsRestart)
    }

    /// True while something runs on its own.
    pub fn running(&self) -> bool {
        matches!(self.phase, Phase::Looking | Phase::FixingUser | Phase::Password | Phase::Verifying)
    }

    /// The stage the file should hold after this state (None: leave it).
    pub fn stage(&self) -> Option<SetupStage> {
        match self.phase {
            Phase::Ready => Some(SetupStage::Done),
            Phase::NeedsRestart => Some(SetupStage::Restart),
            _ => None,
        }
    }

    /// Give the plan text (`root-plan.txt`): the steps of the root job are listed as waiting before it runs.
    pub fn set_plan(&mut self, text: &str) {
        self.plan_steps = parse_steps(text);
        if self.root_steps.is_empty() {
            self.root_steps = self.plan_steps.clone();
        }
        if self.raw.is_empty() {
            self.raw = text.to_string();
        }
    }

    /// Start a run. `root_job`: the installer's root job is still to do (first run, or run again).
    pub fn start(&mut self, kind: RunKind, root_job: bool) -> Action {
        let plan = self.plan_steps.clone();
        let raw_plan = if self.plan_steps.is_empty() { String::new() } else { std::mem::take(&mut self.raw) };
        *self = AutoSetup { kind, root_job, plan_steps: plan.clone(), root_steps: plan, raw: raw_plan, ..AutoSetup::default() };
        self.phase = Phase::Looking;
        self.stages[0] = StepState::Running;
        if kind.check_only() {
            // only the checks: the first three stages were done before the restart (or before this update)
            self.stages = [StepState::Done, StepState::Done, StepState::Done, StepState::Running];
            self.phase = Phase::Verifying;
        }
        Action::RunChecks
    }

    /// Show the first screen. Detection only pre-selects (`pre`); nothing detected: nothing is selected.
    pub fn ask_headset(&mut self, kind: RunKind, pre: Option<(HeadsetPick, String)>) {
        let (selected, found) = pre.map_or((HeadsetPick::Unset, None), |(p, why)| (p, Some(why)));
        *self = AutoSetup { kind, selected, found, phase: Phase::ChooseHeadset, ..AutoSetup::default() };
    }

    /// The person clicked an entry of the list.
    pub fn select_headset(&mut self, p: HeadsetPick) {
        if self.phase == Phase::ChooseHeadset {
            self.selected = p;
        }
    }

    /// "Continue" on the first screen: a listed model goes on to the "Get ready" card (`Some(pick)`), "Other" and
    /// nothing selected do not (the caller starts the setup for Other). Returns the choice to save, None while
    /// nothing is selected.
    pub fn headset_continue(&mut self) -> Option<HeadsetPick> {
        if self.phase != Phase::ChooseHeadset || self.selected == HeadsetPick::Unset {
            return None;
        }
        if self.selected.module().is_some() {
            self.phase = Phase::GetReady;
        }
        Some(self.selected)
    }

    /// "Back" on the "Get ready" card.
    pub fn headset_back(&mut self) {
        if self.phase == Phase::GetReady {
            self.phase = Phase::ChooseHeadset;
        }
    }

    /// Cancel: nothing new starts after the step that runs now.
    pub fn cancel(&mut self) {
        self.cancel = true;
    }

    /// "Later" on the restart screen.
    pub fn restart_later(&mut self) {
        self.later = true;
    }

    fn stopped(&mut self) -> Action {
        self.phase = Phase::Cancelled;
        for s in self.stages.iter_mut().filter(|s| matches!(s, StepState::Running | StepState::Pending)) {
            *s = StepState::Skipped;
        }
        Action::None
    }

    /// The checks are in (stage 1, or stage 4 / the post-restart checks).
    pub fn checks_done(&mut self, checks: Vec<Check>) -> Action {
        self.checks = checks;
        match self.phase {
            Phase::Looking => {
                if self.cancel {
                    return self.stopped();
                }
                self.stages[0] = StepState::Done;
                self.queue = self.checks.iter().filter(|c| c.state == CheckState::Fail && c.fix == CheckFix::User).map(|c| c.id.clone()).collect();
                self.from_problems = false;
                if self.queue.is_empty() {
                    self.stages[1] = StepState::Skipped;
                    return self.password();
                }
                self.phase = Phase::FixingUser;
                self.stages[1] = StepState::Running;
                Action::RunUserFix(self.queue.remove(0))
            }
            Phase::Verifying => self.verified(),
            _ => Action::None,
        }
    }

    /// A user fix ended (its success does not matter: stage 4 checks the result).
    pub fn user_fix_done(&mut self, _ok: bool) -> Action {
        if self.phase != Phase::FixingUser {
            return Action::None;
        }
        if self.cancel {
            return self.stopped();
        }
        if !self.queue.is_empty() {
            return Action::RunUserFix(self.queue.remove(0));
        }
        if self.from_problems {
            return self.verify_again();
        }
        self.stages[1] = StepState::Done;
        self.password()
    }

    fn verify_again(&mut self) -> Action {
        self.phase = Phase::Verifying;
        self.stages[3] = StepState::Running;
        Action::RunChecks
    }

    /// Stage 3: the root job, or the batch of root fixes, or nothing.
    fn password(&mut self) -> Action {
        if self.cancel {
            return self.stopped();
        }
        let root_ids: Vec<String> = self.checks.iter().filter(|c| c.state == CheckState::Fail && matches!(c.fix, CheckFix::Root { .. })).map(|c| c.id.clone()).collect();
        let user_ran = self.stages[1] == StepState::Done;
        if self.root_job {
            self.phase = Phase::Password;
            self.stages[2] = StepState::Running;
            return Action::RunRoot(RootPlan::Setup);
        }
        if !root_ids.is_empty() {
            self.phase = Phase::Password;
            self.stages[2] = StepState::Running;
            return Action::RunRoot(RootPlan::Fixes(root_ids));
        }
        self.stages[2] = StepState::Skipped;
        if user_ran {
            return self.verify_again();
        }
        // nothing ran since the first collection: those checks are the final ones
        self.phase = Phase::Verifying;
        self.stages[3] = StepState::Running;
        self.verified()
    }

    /// Feed the root job's output (all of it so far): the live list moves.
    pub fn root_output(&mut self, text: &str) {
        self.raw = text.to_string();
        let mut steps = self.plan_steps.clone();
        apply_progress(&mut steps, text);
        self.root_steps = steps;
        if text.lines().any(|l| l.trim() == "REBOOT=yes") {
            self.reboot = true;
        }
    }

    /// The root job ended.
    pub fn root_finished(&mut self, outcome: RootOutcome) -> Action {
        if self.phase != Phase::Password {
            return Action::None;
        }
        match outcome {
            RootOutcome::Ok { reboot } => {
                self.reboot |= reboot;
                finish_steps(&mut self.root_steps, false);
                self.stages[2] = StepState::Done;
                self.root_job = false;
                self.verify_again()
            }
            RootOutcome::Cancelled => {
                self.stages[2] = StepState::Failed;
                self.phase = Phase::PasswordStopped(Stop::Cancelled);
                Action::None
            }
            RootOutcome::WrongPassword => {
                self.stages[2] = StepState::Failed;
                self.phase = Phase::PasswordStopped(Stop::WrongPassword);
                Action::None
            }
            RootOutcome::NoHeadsetDisplay => {
                // only the EDID step was skipped (it goes last; everything before it was done): never offer the
                // whole job again, the display-mode check carries its own one-step Fix
                finish_steps(&mut self.root_steps, true);
                self.stages[2] = StepState::Done;
                self.root_job = false;
                self.edid_skipped = true;
                self.verify_again()
            }
            RootOutcome::Failed(why) => self.root_failed(why),
        }
    }

    /// The app could not ask for the password at all.
    pub fn no_password_dialog(&mut self) {
        if self.phase == Phase::Password {
            self.stages[2] = StepState::Failed;
            self.phase = Phase::PasswordStopped(Stop::NoDialog);
        }
    }

    fn root_failed(&mut self, why: String) -> Action {
        finish_steps(&mut self.root_steps, true);
        self.stages[2] = StepState::Failed;
        self.root_error = Some(why);
        self.root_job = false;
        self.verify_again()
    }

    /// Stage 4 (or the post-restart checks) is in.
    fn verified(&mut self) -> Action {
        self.stages[3] = StepState::Done;
        let mut problems: Vec<Check> = self.checks.iter().filter(|c| c.state == CheckState::Fail).cloned().collect();
        let waits = self.checks.iter().any(|c| c.state == CheckState::WaitsForRestart);
        if let Some(why) = &self.root_error {
            // the job failed: its own line first, with a retry
            problems.insert(0, Check { id: "root-job".into(), title: "System setup".into(), state: CheckState::Fail, reason: why.clone(), detail: why.clone(), fix: CheckFix::Root { reboot: false } });
        }
        if self.edid_skipped {
            for p in problems.iter_mut().filter(|p| p.id == "connector") {
                p.reason = format!("The headset's display did not show up while the system setup ran, so only this step is left. {}", p.reason);
            }
        }
        self.problems = problems;
        if !self.problems.is_empty() {
            self.stages[3] = StepState::Failed;
            self.phase = Phase::Problems;
            return Action::None;
        }
        self.phase = if !self.kind.check_only() && (self.reboot || waits) || (self.kind.check_only() && waits) {
            self.reboot = true;
            Phase::NeedsRestart
        } else {
            Phase::Ready
        };
        Action::None
    }

    /// The Fix button of one problem.
    pub fn fix_one(&mut self, id: &str) -> Action {
        let Some(p) = self.problems.iter().find(|p| p.id == id).cloned() else { return Action::None };
        match p.fix {
            CheckFix::User => {
                self.phase = Phase::FixingUser;
                self.from_problems = true;
                self.queue.clear();
                self.stages[3] = StepState::Pending;
                Action::RunUserFix(p.id)
            }
            CheckFix::Root { .. } => self.fix_root(vec![p.id]),
            CheckFix::None => Action::None,
        }
    }

    /// "Fix all (one password)": every root fix in one prompt (the user ones first).
    pub fn fix_all(&mut self) -> Action {
        let root: Vec<String> = self.problems.iter().filter(|p| matches!(p.fix, CheckFix::Root { .. })).map(|p| p.id.clone()).collect();
        if root.is_empty() {
            if let Some(u) = self.problems.iter().find(|p| p.fix == CheckFix::User).map(|p| p.id.clone()) {
                return self.fix_one(&u);
            }
            return Action::None;
        }
        self.fix_root(root)
    }

    fn fix_root(&mut self, ids: Vec<String>) -> Action {
        self.root_error = None;
        self.phase = Phase::Password;
        self.stages[2] = StepState::Running;
        self.stages[3] = StepState::Pending;
        self.root_steps.clear();
        self.raw.clear();
        self.root_job = false;
        // a failed installer job is retried as the whole job
        if ids.iter().any(|i| i == "root-job") {
            self.root_job = true;
            self.root_steps = self.plan_steps.clone();
            return Action::RunRoot(RootPlan::Setup);
        }
        Action::RunRoot(RootPlan::Fixes(ids))
    }

    /// "Check again" (a problem only the person can fix, like a cable): the checks run again.
    pub fn check_again(&mut self) -> Action {
        self.root_error = None;
        self.verify_again()
    }

    /// "Try again" after the password step stopped: the whole run again.
    pub fn retry(&mut self) -> Action {
        let kind = if self.kind.check_only() { self.kind } else { RunKind::Again };
        let root_job = !self.plan_steps.is_empty() && kind == RunKind::Again;
        self.start(kind, root_job)
    }

    /// The step the live list shows as running (for the one-line summary).
    pub fn current_step(&self) -> Option<&RootStep> {
        self.root_steps.iter().find(|s| s.state == StepState::Running)
    }

    /// The one-line summary of the run.
    pub fn headline(&self) -> String {
        match &self.phase {
            Phase::Idle => "Getting ready...".into(),
            Phase::ChooseHeadset => "Which headset do you have?".into(),
            Phase::GetReady => "Get the headset ready, then start the setup.".into(),
            Phase::Looking => "Looking at this PC...".into(),
            Phase::FixingUser => "Fixing what needs no password...".into(),
            Phase::Password => "Setting up the system. Type your password when asked, once.".into(),
            Phase::Verifying if self.kind == RunKind::AfterReboot => "Checking after the restart...".into(),
            Phase::Verifying if self.kind == RunKind::AfterUpdate => "Checking after the update...".into(),
            Phase::Verifying => "Checking everything again...".into(),
            Phase::NeedsRestart => "Everything is set up. One restart is left.".into(),
            Phase::Problems => format!("{} thing{} to fix", self.problems.len(), if self.problems.len() == 1 { "" } else { "s" }),
            Phase::PasswordStopped(Stop::Cancelled) => "The password window was closed. Nothing was changed.".into(),
            Phase::PasswordStopped(Stop::WrongPassword) => "The password was not accepted. Nothing was changed.".into(),
            Phase::PasswordStopped(Stop::NoDialog) => "No password window could be opened. Nothing was changed.".into(),
            Phase::Cancelled => "Setup was cancelled. Nothing more was changed.".into(),
            Phase::Ready => "Your PC is ready.".into(),
        }
    }
}

/// Mark the steps when the job has ended: the one that ran last is done (or failed), the ones never reached are skipped.
fn finish_steps(steps: &mut [RootStep], failed: bool) {
    for s in steps.iter_mut() {
        match s.state {
            StepState::Running => s.state = if failed { StepState::Failed } else { StepState::Done },
            StepState::Pending if !failed => {
                s.state = StepState::Skipped;
                if s.note.is_empty() {
                    s.note = "not needed on this PC".into();
                }
            }
            _ => {}
        }
    }
}

/// The steps of a plan text (`omarchyvr-root.sh plan`): all waiting.
pub fn parse_steps(text: &str) -> Vec<RootStep> {
    let mut steps = Vec::new();
    apply_progress(&mut steps, text);
    for s in &mut steps {
        if s.state == StepState::Running || s.state == StepState::Done {
            s.state = StepState::Pending;
        }
    }
    steps
}

/// A step line of the job: `" 3. Text"`.
fn step_line(l: &str) -> Option<(u32, &str)> {
    let t = l.trim_start();
    let (n, rest) = t.split_once(". ")?;
    if n.is_empty() || n.len() > 3 || !n.chars().all(|c| c.is_ascii_digit()) || l.starts_with("      ") {
        return None;
    }
    Some((n.parse().ok()?, rest.trim()))
}

/// Move the live list along the job's output: a step line makes the step running and the ones before it done; the
/// indented lines under it are its notes (and tell when it was skipped).
pub fn apply_progress(steps: &mut Vec<RootStep>, text: &str) {
    let mut cur: Option<usize> = None;
    for l in text.lines() {
        if let Some((n, title)) = step_line(l) {
            if let Some(c) = cur {
                if steps[c].state == StepState::Running {
                    steps[c].state = StepState::Done;
                }
            }
            let idx = match steps.iter().position(|s| s.n == n) {
                Some(i) => {
                    steps[i].raw = title.to_string();
                    steps[i].title = friendly(title);
                    i
                }
                None => {
                    steps.push(RootStep { n, title: friendly(title), raw: title.to_string(), state: StepState::Pending, note: String::new() });
                    steps.len() - 1
                }
            };
            steps[idx].state = StepState::Running;
            steps[idx].note.clear();
            if title.contains("NOT applied") || title.contains("no driver patch") {
                steps[idx].note = if title.contains("NOT tested") { "this driver version is not tested; allow it under NVIDIA driver patch to patch it".into() } else { "not needed on this PC".into() };
                steps[idx].state = StepState::Skipped;
            }
            cur = Some(idx);
        } else if let (Some(c), true) = (cur, l.starts_with("      ")) {
            let t = l.trim();
            if t.starts_with("would:") || t.is_empty() {
                continue;
            }
            if t.contains("headset display not seen") {
                steps[c].state = StepState::Skipped;
                steps[c].note = "the headset's display was not visible; turn the headset on and press Fix".into();
            } else if steps[c].note.is_empty() && !t.starts_with("wrote ") && !t.starts_with("unchanged") {
                steps[c].note = t.to_string();
            }
        }
    }
}

/// Plain-words titles for the steps of the root job (unknown ones keep the job's own text, cut before the first colon).
pub fn friendly(raw: &str) -> String {
    let r = raw.trim();
    let table: [(&str, &str); 9] = [
        ("Restore point", "Save a restore point of your system"),
        ("Packages from the Arch", "Install missing packages"),
        ("Headset access", "Let your user open the headset"),
        ("Uninstall helper", "Keep a safe copy of the setup script"),
        ("NVIDIA: no driver patch", "NVIDIA driver patch (not needed here)"),
        ("Headset display mode", "Set the headset's display mode for the next start"),
        ("Realtime priority for the VR", "Allow realtime priority for VR"),
        ("SteamVR's compositor", "Allow realtime priority for SteamVR"),
        ("NVIDIA", "Patch the NVIDIA driver for the headset"),
    ];
    for (p, f) in table {
        if r.starts_with(p) {
            if p == "NVIDIA" && r.contains("NOT tested") {
                return "NVIDIA driver (version not tested: not patched)".into();
            }
            return f.to_string();
        }
    }
    match r.split_once(':') {
        Some((a, _)) if a.len() >= 4 => a.to_string(),
        _ => r.to_string(),
    }
}

/// The exit of the root job, as the person should hear it.
pub fn outcome_of(result: &Result<(), String>, output: &str) -> RootOutcome {
    match result {
        Ok(()) => RootOutcome::Ok { reboot: output.lines().any(|l| l.trim() == "REBOOT=yes") },
        Err(e) => {
            if e.contains("exit code 126") {
                RootOutcome::Cancelled
            } else if e.contains("exit code 127") {
                RootOutcome::WrongPassword
            } else if e.contains("exit code 3") && output.contains("headset display not seen") {
                RootOutcome::NoHeadsetDisplay
            } else {
                let last = output.lines().rev().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("REBOOT=")).unwrap_or("");
                RootOutcome::Failed(if last.is_empty() { e.clone() } else { format!("{last} ({e})") })
            }
        }
    }
}

// ---- checks from the checklist -------------------------------------------------------------------------------------

/// Where the EDID override lives (written by the root job's EDID step).
pub const EDID_FILES: [&str; 2] = ["/etc/pimax/dp-edid.bin", "/etc/modprobe.d/nvidia-pimax.conf"];

fn plain(id: &str) -> (&'static str, &'static str) {
    match id {
        "headset" => ("Headset found on USB", "The headset is not seen on USB. Plug it in (USB-C and DisplayPort) and turn it on."),
        "headset-camera" => ("Headset cameras", "The headset is on, but its cameras are not on USB. The setup restarts the headset once; if they stay off, the headset needs a power cycle: follow the steps shown here (the power button first)."),
        "eye-chip" => ("Eye tracker", "The eye tracker is switched off (normal after the headset restarted). Fix switches it on."),
        "udev" | "udev-rule" => ("Permission to use the headset", "Your user may not open the headset yet."),
        "keepalive" => ("Headset keep-alive", "The service that keeps the headset display awake is not running."),
        "pacman-hooks" => ("Driver updates keep the patch", "A driver update could drop the NVIDIA patch."),
        "modinfo" => ("NVIDIA driver is patched", "The installed NVIDIA driver is not patched for the headset."),
        "merged-head-sync" => ("Patched NVIDIA driver is loaded", "The patched NVIDIA driver is not the one running."),
        "connector" => ("Headset display mode", "The headset's display mode is not set up yet."),
        "setcap-desk" => ("Realtime priority for VR", "VR does not have realtime priority yet (a changed Monado loses it: the kernel drops it when the file is replaced). One password sets it again."),
        "gpu" => ("Graphics card", "The graphics card is not supported or not detected."),
        "lease" => ("Desktop can hand the display to VR", "This desktop cannot hand a display to VR (wp_drm_lease_device_v1)."),
        _ => ("Check", "Something is not right."),
    }
}

/// The checks the setup shows, from the checklist.
///
/// * `edid_fix`: the root job's EDID mode can be run (an installed plan exists): the display-mode check gets a Fix.
/// * `present`: whether the EDID override files exist (decides "waits for the restart" or "missing").
pub fn checks_from_items(items: &[Item], edid_fix: bool, present: &dyn Fn(&Path) -> bool) -> Vec<Check> {
    let get = |id: &str| items.iter().find(|i| i.id == id);
    let edid_in_place = EDID_FILES.iter().all(|f| present(Path::new(f)));
    let mut out = Vec::new();
    for id in SETUP_IDS {
        let Some(it) = get(id) else { continue };
        let (title, reason) = plain(id);
        let strict = matches!(id, "udev-rule" | "pacman-hooks" | "modinfo" | "setcap-desk" | "udev" | "eye-chip");
        let bad = if strict { it.level >= Level::Warn } else { it.level >= Level::Fail };
        // what the fix would be
        let mut fix = match &it.fix {
            Fix::User(_) => CheckFix::User,
            Fix::Root(r) if r.pkexec.is_ok() => CheckFix::Root { reboot: r.reboot },
            _ => CheckFix::None,
        };
        let mut state = if bad { CheckState::Fail } else { CheckState::Ok };
        let mut reason = reason.to_string();
        match id {
            "merged-head-sync" if bad => {
                if get("modinfo").is_some_and(|m| m.level == Level::Ok) {
                    state = CheckState::WaitsForRestart;
                    reason = "The patched driver is installed and starts working after the restart.".into();
                } else {
                    // the installed module is not patched: the modinfo check carries the fix
                    fix = CheckFix::None;
                    reason = "Waiting for the driver patch (see above).".into();
                }
            }
            "connector" if bad => {
                if it.detail.contains("7104x3840 missing") && edid_in_place {
                    state = CheckState::WaitsForRestart;
                    reason = "The display mode is in place and starts working after the restart.".into();
                } else if it.detail.contains("not connected") {
                    fix = CheckFix::None;
                    reason = "The headset's display is not connected. Plug in its DisplayPort cable and turn the headset on.".into();
                } else {
                    fix = if edid_fix { CheckFix::Root { reboot: true } } else { CheckFix::None };
                    reason = if it.detail.contains("no connector") {
                        "The headset's display is not visible. Turn the headset on, then press Fix: the setup waits up to 30 seconds for it.".into()
                    } else {
                        "The headset shows only desktop modes. Press Fix to set up its display mode (needs a restart).".into()
                    };
                }
            }
            "gpu" | "lease" | "headset" => fix = CheckFix::None,
            _ => {}
        }
        out.push(Check { id: id.to_string(), title: title.to_string(), state, reason: if state == CheckState::Ok { String::new() } else { reason }, detail: it.detail.clone(), fix });
    }
    out
}

/// The headset check in words that fit what USB shows: a sleeping Dream Air (its control chip `34a4:0044` off, its
/// bridge with the cameras or the eye tracker on USB) is not "not plugged in" (real setup 2026-10-17 11:51-13:00: the
/// chip stayed off for an hour; only a power cycle of the headset brought it back).
/// While the control chip is off, the headset's display cannot show, so its display-mode step (the EDID-only root job)
/// would only end in "headset display not seen" (exit 3): its Fix waits for the headset instead of asking for a password
/// (real setup 2026-10-17 12:40-12:52: three EDID-only prompts while the chip was off; none could succeed).
pub fn explain_headset(checks: &mut [Check], usb: &[(u16, u16)]) {
    let state = headset_choice::headset_state_from_usb(usb);
    if state == headset_choice::HeadsetUsb::Awake {
        return;
    }
    for c in checks.iter_mut().filter(|c| c.id == "connector" && c.state == CheckState::Fail && matches!(c.fix, CheckFix::Root { .. })) {
        c.fix = CheckFix::None;
        c.reason = "The headset's display mode is set up once the headset is awake (see above): its display shows only then. The Fix appears here by itself.".into();
    }
    if state != headset_choice::HeadsetUsb::Asleep {
        return;
    }
    for c in checks.iter_mut().filter(|c| c.id == "headset" && c.state == CheckState::Fail) {
        c.title = "Headset is awake".into();
        c.reason = "The headset is asleep or off: its control chip (34a4:0044) is not on USB, only its other parts are. Follow the steps shown here: press the headset's power button; if it stays asleep, the power cable is the last step.".into();
        c.fix = CheckFix::None;
    }
}

/// The checks that depend on the headset being on and awake (its parts and its display).
pub const HEADSET_DEPENDENT: [&str; 4] = ["headset", "headset-camera", "eye-chip", "connector"];

/// The check after an update looks at what an update can change on this PC (the realtime permission of a changed
/// Monado, the access rule, the driver patch), not at the headset: a headset that sleeps while the person updates is
/// normal and must not pull the setup screen up. Its parts are checked by the start and by the setup itself.
pub fn update_checks(checks: &mut Vec<Check>) {
    checks.retain(|c| !HEADSET_DEPENDENT.contains(&c.id.as_str()));
}

/// A run in a fixed state, for the screenshots (`omarchyvr-app --screenshots`) and the demo: it is driven through the
/// real state machine with fake checks and a made-up root job output, nothing runs. `which`: `running`, `restart`,
/// `problem`, `parts-asleep`, `parts-cameras`, `after-update`, `ready-after-reboot`, `stopped`.
pub fn demo_run(which: &str, plan: &str) -> AutoSetup {
    let chk = |id: &str, state: CheckState, fix: CheckFix| {
        let (title, reason) = plain(id);
        Check { id: id.into(), title: title.into(), state, reason: if state == CheckState::Ok { String::new() } else { reason.into() }, detail: format!("{id}: demo"), fix }
    };
    let green = |waits: &[&str]| -> Vec<Check> {
        ["headset", "udev-rule", "keepalive", "modinfo", "merged-head-sync", "connector", "setcap-desk"]
            .iter()
            .map(|id| chk(id, if waits.contains(id) { CheckState::WaitsForRestart } else { CheckState::Ok }, CheckFix::None))
            .collect()
    };
    let output = "omarchyvr system setup (applying):\n 1. Restore point: a snapper snapshot of /\n      snapper -c root create -d 'before omarchyvr install'\n 2. Headset access: udev rule /etc/udev/rules.d/70-pimax.rules\n      wrote /etc/udev/rules.d/70-pimax.rules\n 3. Uninstall helper: /usr/local/lib/omarchyvr/omarchyvr-root.sh\n      wrote /usr/local/lib/omarchyvr/omarchyvr-root.sh\n 4. NVIDIA 610.57.04 (tested): display patches for the headset's two-head mode\n      patch /usr/src/nvidia-610.57.04 (originals kept as *.pimax-orig), dkms build + install\n";
    let mut s = AutoSetup::default();
    s.set_plan(plan);
    match which {
        "choose" => s.ask_headset(RunKind::Fresh, headset_choice::preselect(&[(0x34a4, 0x43), (0x34a4, 0x14), (0x2104, 0x561)])),
        "choose-none" => s.ask_headset(RunKind::Fresh, None),
        "get-ready" => {
            s.ask_headset(RunKind::Fresh, headset_choice::preselect(&[(0x34a4, 0x43), (0x34a4, 0x14), (0x2104, 0x561)]));
            s.headset_continue();
        }
        "ready-after-reboot" => {
            s.start(RunKind::AfterReboot, false);
            s.checks_done(green(&[]));
        }
        "problem" => {
            s.start(RunKind::AfterReboot, false);
            let mut c = green(&[]);
            for x in &mut c {
                if x.id == "connector" {
                    *x = chk("connector", CheckState::Fail, CheckFix::Root { reboot: true });
                }
            }
            s.checks_done(c);
        }
        // 2026-10-17 12:40: the system part ran (a restart is due) while the headset slept: its control chip and the
        // display mode are the problems, the guide shows, Restart now stays
        "parts-asleep" => {
            s.start(RunKind::Fresh, true);
            let mut c = green(&[]);
            for x in &mut c {
                if x.id == "headset" {
                    *x = chk("headset", CheckState::Fail, CheckFix::None);
                }
                if x.id == "connector" {
                    *x = chk("connector", CheckState::Fail, CheckFix::Root { reboot: true });
                }
            }
            s.checks_done(c.clone());
            s.root_output(&format!("{output} 5. Headset display mode at boot: EDID override (connector found after the headset wakes, up to 60 s)\n      headset display not seen: put the headset on / turn it on, then press Fix in the app (EDID override skipped, nothing else is affected)\nREBOOT=yes\n"));
            s.root_finished(RootOutcome::Ok { reboot: true });
            explain_headset(&mut c, &[(0x34a4, 0x43), (0x34a4, 0x14), (0x2104, 0x561)]);
            s.checks_done(c);
        }
        // after the restart the cameras are still off (the setup's one headset restart did not bring them back)
        "parts-cameras" => {
            s.start(RunKind::AfterReboot, false);
            let mut c = green(&[]);
            c.push(chk("headset-camera", CheckState::Fail, CheckFix::User));
            s.checks_done(c);
        }
        // the check after an update: a changed Monado lost its realtime permission
        "after-update" => {
            s.start(RunKind::AfterUpdate, false);
            let mut c = green(&[]);
            for x in &mut c {
                if x.id == "setcap-desk" {
                    *x = chk("setcap-desk", CheckState::Fail, CheckFix::Root { reboot: false });
                }
            }
            s.checks_done(c);
        }
        _ => {
            s.start(RunKind::Fresh, true);
            let mut c = green(&[]);
            c[2] = chk("keepalive", CheckState::Fail, CheckFix::User);
            s.checks_done(c);
            s.user_fix_done(true);
            match which {
                "running" => s.root_output(output),
                "stopped" => {
                    s.root_finished(RootOutcome::Cancelled);
                }
                _ => {
                    s.root_output(&format!("{output} 5. Headset display mode at boot: EDID override (connector found after the headset wakes, up to 30 s)\n      headset display found: [0000:01:00.0].DP-0\n 6. Realtime priority for the VR compositor: cap_sys_nice\n      setcap cap_sys_nice+ep /x/monado-service\nREBOOT=yes\n"));
                    s.root_finished(RootOutcome::Ok { reboot: true });
                    s.checks_done(green(&["merged-head-sync", "connector"]));
                }
            }
        }
    }
    s
}

#[cfg(test)]
#[path = "autosetup_tests.rs"]
mod tests;

// ---- the glue ------------------------------------------------------------------------------------------------------

/// The installer's root job in its EDID-only mode, as a root fix line.
pub fn edid_fix_line(i: &Install) -> String {
    format!("{} edid {}", i.root_script().display(), i.plan_env().display())
}

/// How the glue tracks the job it started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupJob {
    /// The job's label.
    pub label: String,
    /// A root job (else a user fix).
    pub root: bool,
    /// Where its output starts in the app log.
    pub log_from: usize,
}

impl AppCore {
    /// Start the run by itself when the app shows the setup screen (once the facts and the install state are read).
    /// A fresh setup begins with "Which headset do you have?" until a headset is chosen.
    pub fn want_setup(&mut self) {
        self.want_onboarding();
        let s = &self.onboarding.setup;
        if s.phase != Phase::Idle || self.demo || self.state.setup == SetupStage::Done || !self.onboarding.install.loaded || self.facts.at == 0 {
            return;
        }
        let kind = if self.state.setup == SetupStage::Restart { RunKind::AfterReboot } else { RunKind::Fresh };
        self.setup_begin(kind);
    }

    /// "Run setup again" (and "Try again" after a stopped password step).
    pub fn setup_rerun(&mut self) {
        if self.onboarding.setup.running() {
            return;
        }
        self.setup_begin(RunKind::Again);
    }

    /// The first screen when no headset is chosen yet (not after a restart); else the run itself.
    fn setup_begin(&mut self, kind: RunKind) {
        if kind != RunKind::AfterReboot && self.state.headset == HeadsetPick::Unset {
            self.onboarding.setup.ask_headset(kind, headset_choice::preselect(&self.facts.usb));
            return;
        }
        self.setup_start(kind);
    }

    /// An entry of the headset list was clicked.
    pub fn setup_select_headset(&mut self, p: HeadsetPick) {
        self.onboarding.setup.select_headset(p);
    }

    /// "Continue" on the first screen: the choice is saved (shared with the CLI); a listed model shows the "Get ready"
    /// card, "Other" starts the setup without headset-specific system changes.
    pub fn setup_headset_continue(&mut self) {
        let kind = self.onboarding.setup.kind;
        let Some(p) = self.onboarding.setup.headset_continue() else { return };
        self.state.headset = p;
        self.write_state();
        if self.onboarding.setup.phase != Phase::GetReady {
            self.setup_start(kind);
        } else {
            self.onboarding.wake.reset();
            self.onboarding.wake_asked = 0.0;
        }
    }

    /// What the wake guidance shows now, and the animation time (the demo can fix both).
    pub fn wake_view(&self, ui_time: f64) -> (crate::wake::Wake, f64) {
        match self.onboarding.wake_demo {
            Some((w, t)) => (w, t),
            None => (self.onboarding.wake.view(Self::wake_now()), ui_time),
        }
    }

    /// Wall-clock seconds for the wake guide's timers.
    pub fn wake_now() -> f64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
    }

    /// After an update by `./install.sh` (its `post-install-check` file): the setup's checks run once by themselves.
    /// All green: nothing is shown and the file goes. A problem (for example the realtime permission of a changed
    /// Monado): the setup screen opens with it and its one-password Fix, before anyone presses Play (real update
    /// 2026-10-17 13:22: the password was asked by the start of Work instead).
    pub(crate) fn post_install_poll(&mut self) {
        if self.demo || self.state.setup != SetupStage::Done || self.onboarding.post_install_started || self.facts.at == 0 {
            return;
        }
        if !self.onboarding.install.loaded {
            self.refresh_install();
            return;
        }
        if !self.onboarding.install.post_install || self.onboarding.setup.phase != Phase::Idle || self.job.is_some() {
            return;
        }
        self.onboarding.post_install_started = true;
        self.setup_start(RunKind::AfterUpdate);
    }

    /// Which "Get the headset ready" guide is on screen: `Some(need_camera)`, or None.
    pub fn wake_target(&self) -> Option<bool> {
        if let Some(w) = &self.onboarding.wake_only {
            return Some(w.need_camera);
        }
        let s = &self.onboarding.setup;
        match s.phase {
            Phase::GetReady if s.selected.module().is_some_and(|m| m.id() == "dream-air") => Some(false),
            Phase::Problems => guide_for(&s.problems),
            _ => None,
        }
    }

    /// Open the guide on its own (a Work/Play start or a headset restart found the headset asleep or its cameras gone).
    /// `reason` says why in plain words; Back returns to the page it came from.
    pub fn open_wake_guide(&mut self, reason: &str, need_camera: bool) {
        let back = if self.page == crate::app::Page::Onboarding { crate::app::Page::Desk } else { self.page };
        self.onboarding.wake_only = Some(crate::onboarding::WakeOnly { back, reason: reason.to_string(), need_camera });
        self.page = crate::app::Page::Onboarding;
    }

    /// Back from the guide opened on its own.
    pub fn close_wake_guide(&mut self) {
        if let Some(w) = self.onboarding.wake_only.take() {
            self.page = w.back;
        }
    }

    /// A failed job that the guide answers: the start's camera recovery or a headset restart ended with the power-cycle
    /// hint, or the headset's control chip was not on USB at all.
    pub fn wake_after_failure(&mut self, text: &str) {
        if text.contains(omarchyvr_tools::vr::reset::POWER_CYCLE_HINT) || text.contains("camera recovery failed") {
            self.open_wake_guide(
                "The headset's cameras did not come back after one restart, so VR with head tracking cannot start. The headset needs a power cycle: follow the steps below. When the tick turns green, go back and start again.",
                true,
            );
        } else if text.contains("IF0 hidraw node of 34a4:0044 not found") {
            self.open_wake_guide("The headset is asleep or off: its control chip is not on USB, so it cannot be restarted from here. Follow the steps below.", false);
        }
    }

    /// Feed the wake guide from the latest USB facts while the "Get ready" card is open for the Dream Air; ask for
    /// fresh USB facts every 3 s until the HID chip is seen.
    pub(crate) fn wake_poll(&mut self) {
        // the result screen waits for the headset's display (EDID step skipped, nothing seen yet): look again every
        // 5 s so the one Fix appears the moment the display does, with no click. While the headset itself sleeps the
        // guide below watches USB instead and checks again once the parts are back (a full check every 5 s would also
        // restart the guide's timers).
        if self.onboarding.setup.phase == Phase::Problems
            && !self.demo
            && self.onboarding.setup_wait_facts.is_none()
            && self.onboarding.setup.problems.iter().any(|p| p.id == "connector" && p.fix == CheckFix::None)
            && guide_for(&self.onboarding.setup.problems).is_none()
            && Self::wake_now() - self.onboarding.wake_asked >= 5.0
        {
            self.onboarding.wake_asked = Self::wake_now();
            let a = self.onboarding.setup.check_again();
            self.setup_exec(a);
            return;
        }
        let Some(need_camera) = self.wake_target() else {
            // a check that runs now keeps the guide where it is (it shows again with the same timers); a finished run
            // forgets it
            if !self.onboarding.setup.running() {
                self.onboarding.wake_for = None;
            }
            return;
        };
        if self.onboarding.wake_for != Some(need_camera) {
            // another guide (the control chip, or the cameras too): start it over, the tick of the other one latched
            self.onboarding.wake.reset();
            self.onboarding.wake_for = Some(need_camera);
            self.onboarding.wake_rechecked = false;
        }
        let now = Self::wake_now();
        let usb = headset_choice::guide_state(&self.facts.usb, need_camera);
        let session = self.live.service.is_some() || self.live.steamvr.is_some();
        self.onboarding.wake.tick(now, usb, session);
        if !self.onboarding.wake.is_awake() && now - self.onboarding.wake_asked >= 3.0 && !self.demo {
            self.onboarding.wake_asked = now;
            self.refresh(false);
        }
        // the setup's problem list: the parts are back, so the checks run again by themselves (once per wake)
        if self.onboarding.setup.phase == Phase::Problems && self.onboarding.wake.is_awake() && !self.onboarding.wake_rechecked && !self.demo {
            self.onboarding.wake_rechecked = true;
            let a = self.onboarding.setup.check_again();
            self.setup_exec(a);
        }
    }

    /// "I unplugged it" (step B).
    pub fn wake_unplugged_pressed(&mut self) {
        self.onboarding.wake.unplugged_pressed(Self::wake_now());
    }

    /// "Try again" after the wake warning.
    pub fn wake_retry(&mut self) {
        self.onboarding.wake.retry(Self::wake_now());
    }

    /// "Start setup" on the "Get ready" card (enabled even when a tick is missing).
    pub fn setup_get_ready_start(&mut self) {
        if self.onboarding.setup.phase == Phase::GetReady {
            let kind = self.onboarding.setup.kind;
            self.setup_start(kind);
        }
    }

    /// The ticks of the "Get ready" card, from what the PC sees now.
    pub fn headset_ready_steps(&self) -> Vec<headset_choice::ReadyStep> {
        let connector = self.facts.doctor_level("connector").map(|l| l == Level::Ok);
        headset_choice::ready_steps(self.onboarding.setup.selected, &self.facts.usb, connector)
    }

    fn setup_start(&mut self, kind: RunKind) {
        let root_job = self.onboarding.install.plan_text.is_some() && !kind.check_only() && (kind == RunKind::Again || !self.onboarding.install.root_done);
        if root_job && !self.demo {
            // the plan is made again from the person's headset choice right before the password step (the plan from
            // ./install.sh can be stale: a sleeping headset hid 34a4:0044 then), with the NVIDIA switch
            let usb = headset_choice::headset_state_from_usb(&self.facts.usb);
            match crate::install::prepare_plan(&self.install(), self.state.nvidia_untested, self.state.headset, usb) {
                Ok(Some(t)) => self.onboarding.install.plan_text = Some(t),
                Ok(None) => {}
                Err(e) => self.message = format!("cannot prepare the setup plan: {e}"),
            }
        }
        if let Some(t) = self.onboarding.install.plan_text.clone() {
            self.onboarding.setup.set_plan(&t);
        }
        let a = self.onboarding.setup.start(kind, root_job);
        self.setup_exec(a);
    }

    /// "Try again" after the password step stopped.
    pub fn setup_retry(&mut self) {
        let a = self.onboarding.setup.retry();
        self.setup_exec(a);
    }

    /// Cancel.
    pub fn setup_cancel(&mut self) {
        self.onboarding.setup.cancel();
        if !self.onboarding.setup.running() {
            return;
        }
        self.message = "Stopping after the step that runs now.".into();
    }

    /// The Fix button of one problem.
    pub fn setup_fix_one(&mut self, id: &str) {
        let a = self.onboarding.setup.fix_one(id);
        self.setup_exec(a);
    }

    /// "Fix all (one password)".
    pub fn setup_fix_all(&mut self) {
        let a = self.onboarding.setup.fix_all();
        self.setup_exec(a);
    }

    /// "Check again".
    pub fn setup_check_again(&mut self) {
        let a = self.onboarding.setup.check_again();
        self.setup_exec(a);
    }

    /// "Restart now": `systemctl reboot` as the user (logind allows it; no password).
    pub fn setup_restart_now(&mut self) {
        let c = crate::cmd::Cmd::new("systemctl", &["reboot"]);
        if self.demo {
            self.message = "demo mode: would restart the PC".into();
            return;
        }
        self.state.setup = SetupStage::Restart;
        self.write_state();
        self.spawn(crate::job::JobSpec { kind: crate::job::JobKind::Other("restart".into()), label: "Restart the PC".into(), steps: vec![crate::job::Step::Run(c)] });
    }

    /// "Later" on the restart screen.
    pub fn setup_restart_later(&mut self) {
        self.onboarding.setup.restart_later();
    }

    /// Collect the checks from the latest facts.
    fn setup_collect(&self) -> Vec<Check> {
        let i = self.install();
        let edid_fix = i.plan_env().is_file() && i.root_script().is_file();
        let mut checks = checks_from_items(&self.items, edid_fix, &|p| self.facts.has(p));
        explain_headset(&mut checks, &self.facts.usb);
        if self.onboarding.setup.kind == RunKind::AfterUpdate {
            update_checks(&mut checks);
        }
        checks
    }

    /// New facts arrived: the stage that waits for checks goes on.
    pub(crate) fn setup_facts(&mut self) {
        let Some(asked) = self.onboarding.setup_wait_facts else { return };
        if self.facts.report.is_none() || self.facts.at < asked {
            return;
        }
        self.onboarding.setup_wait_facts = None;
        let checks = self.setup_collect();
        let a = self.onboarding.setup.checks_done(checks);
        self.setup_after();
        self.setup_exec(a);
    }

    /// Keep the saved stage and the onboarded flag in line with the run.
    fn setup_after(&mut self) {
        // the check after an update: a problem opens the setup screen with its Fix at once (once); all green removes
        // install.sh's request
        if self.onboarding.setup.kind == RunKind::AfterUpdate && self.onboarding.setup.phase == Phase::Problems && !self.onboarding.post_install_shown {
            self.onboarding.post_install_shown = true;
            self.page = crate::app::Page::Onboarding;
        }
        if self.onboarding.setup.phase == Phase::Ready && self.onboarding.install.post_install && !self.demo {
            let f = self.install().post_install_check();
            if let Err(e) = std::fs::remove_file(&f) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    log::warn!("cannot remove {}: {e}", f.display());
                }
            }
            self.onboarding.install.post_install = false;
        }
        if let Some(st) = self.onboarding.setup.stage() {
            if self.state.setup != st || (st == SetupStage::Done && !self.state.onboarded) {
                self.state.setup = st;
                if st == SetupStage::Done {
                    self.state.onboarded = true;
                }
                self.write_state();
            }
        }
    }

    /// Feed the root job's live output (called every poll while it runs).
    pub(crate) fn setup_poll(&mut self) {
        let Some(j) = self.onboarding.setup_job.clone() else { return };
        if !j.root || self.onboarding.setup.phase != Phase::Password {
            return;
        }
        let from = j.log_from.min(self.log.len());
        let text = self.log[from..].join("\n");
        if text != self.onboarding.setup.raw {
            self.onboarding.setup.root_output(&text);
        }
    }

    /// A job ended: if it is the setup's, the machine goes on.
    pub(crate) fn setup_job_finished(&mut self, label: &str, r: &Result<(), String>) {
        let Some(j) = self.onboarding.setup_job.clone() else { return };
        if j.label != label {
            return;
        }
        self.onboarding.setup_job = None;
        let a = if j.root {
            let from = j.log_from.min(self.log.len());
            let text = self.log[from..].join("\n");
            self.onboarding.setup.root_output(&text);
            let outcome = outcome_of(r, &text);
            self.onboarding.setup.root_finished(outcome)
        } else {
            self.onboarding.setup.user_fix_done(r.is_ok())
        };
        self.setup_exec(a);
    }

    /// Run what the machine asks for.
    fn setup_exec(&mut self, a: Action) {
        match a {
            Action::None => self.setup_after(),
            Action::RunChecks => {
                self.onboarding.setup_wait_facts = Some(self.machine.now_unix());
                self.refresh(true);
            }
            Action::RunUserFix(id) => {
                self.run_user_fix(&id);
                match self.job.as_ref().map(|j| j.label.clone()) {
                    Some(label) => self.onboarding.setup_job = Some(SetupJob { label, root: false, log_from: self.log.len() }),
                    None => {
                        // not started (VR runs, another job, nothing to run): the next stage looks at the result
                        let next = self.onboarding.setup.user_fix_done(false);
                        self.setup_exec(next);
                    }
                }
            }
            Action::RunRoot(plan) => self.setup_run_root(plan),
        }
    }

    fn setup_run_root(&mut self, plan: RootPlan) {
        if self.demo {
            return;
        }
        let i = self.install();
        let (title, fix) = match &plan {
            RootPlan::Setup => {
                let text = self.onboarding.install.plan_text.clone().unwrap_or_default();
                ("Set up this PC".to_string(), setup_fix(&i, &text))
            }
            RootPlan::Fixes(ids) => {
                let mut lines: Vec<String> = Vec::new();
                let mut reboot = false;
                for id in ids {
                    let l = if id == "connector" {
                        reboot = true;
                        vec![edid_fix_line(&i)]
                    } else {
                        match self.item(id).map(|x| x.fix.clone()) {
                            Some(Fix::Root(r)) => {
                                reboot |= r.reboot;
                                r.lines
                            }
                            _ => continue,
                        }
                    };
                    for x in l {
                        if !lines.contains(&x) {
                            lines.push(x);
                        }
                    }
                }
                // the EDID step waits for the headset and may end with exit 3: it goes last
                lines.sort_by_key(|l| l.contains(" edid "));
                ("Fix the setup".to_string(), crate::checklist::RootFix::new("Fix what the setup found. One password.", lines, vec![], reboot))
            }
        };
        if !self.facts.polkit_agent {
            self.onboarding.setup.no_password_dialog();
            return;
        }
        let log_from = self.log.len();
        self.pending = Some(Pending::Root { title: title.clone(), fix, catalogue: None });
        self.confirm_root();
        match self.job.as_ref().map(|j| j.label.clone()) {
            Some(label) => self.onboarding.setup_job = Some(SetupJob { label, root: true, log_from }),
            None => {
                let why = self.message.clone();
                let a = self.onboarding.setup.root_finished(RootOutcome::Failed(if why.is_empty() { "the setup could not start".into() } else { why }));
                self.setup_exec(a);
            }
        }
    }
}
