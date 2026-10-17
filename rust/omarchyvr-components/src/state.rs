//! Actual state: the [`World`] folds raw observations ([`crate::probe::Obs`]), exits, kernel lines and executor
//! results into debounced per-component state, derives health per component and the overall summary, and keeps the
//! restart bookkeeping the reconciler reads. Pure: the caller passes the time and the observations, so the daemon,
//! the simulator and the tests drive the same code.
//!
//! Adoption: a Monado process omarchyvrd has not seen before sets the intent to on (`by=adopted`) and starts a new
//! session generation. In an adopted session a process counts as wanted only once it has been seen up (omarchyvrd never
//! starts what the launcher did not start); a process that exits the user's way (exit 0, SIGINT/SIGTERM, the
//! launcher's stop) is turned off for the session; a crash is remediated within budgets.
use crate::gpu::Gate;
use crate::model::{By, Class, Desired, GateLevel, Health, Id, Intent, Kind, Mode, Overall};
use crate::policy::{classify_exit, Budgets, ExitClass, ExitFacts, Global, RestartState, COLLATERAL_S};
use crate::probe::{Obs, SessionFacts};
use crate::spec::{spec, Scope, Spec, SPECS};
use std::collections::BTreeMap;

/// Per-component runtime state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Comp {
    /// Debounced presence / liveness (None: unknown).
    pub up: Option<bool>,
    /// The last raw observation and since when it has held.
    pub raw: Option<bool>,
    pub raw_since: f64,
    /// When `up` last changed.
    pub since: f64,
    pub pid: Option<u32>,
    pub start_ticks: Option<u64>,
    pub ready: Option<bool>,
    pub degraded: Option<String>,
    pub detail: String,
    pub dstate: bool,
    /// Since when it has been in D state.
    pub dstate_since: Option<f64>,
    /// Seen up during this session generation.
    pub seen_up: bool,
    /// omarchyvrd started it: ready by then.
    pub starting_until: Option<f64>,
    /// omarchyvrd is stopping it (its exit is expected).
    pub stopping: bool,
    /// Last exit: (time, class, text).
    pub last_exit: Option<(f64, ExitClass, String)>,
    /// When it went down (for backoff and collateral).
    pub down_at: Option<f64>,
    pub restart: RestartState,
    /// L5 notices already given (by key) this generation.
    pub told: Vec<String>,
}

/// How an executed step ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Ok,
    Failed(String),
    /// A stop left the process alive (Monado: the gate goes to needs-reboot; nothing is killed).
    Hung(String),
}

/// Something worth an event line or a notification.
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub id: String,
    pub text: String,
    /// Also a desktop notification / headset toast.
    pub notify: bool,
    pub critical: bool,
}

impl Notice {
    pub fn log(id: &str, text: impl Into<String>) -> Notice {
        Notice { id: id.into(), text: text.into(), notify: false, critical: false }
    }
    pub fn tell(id: &str, text: impl Into<String>) -> Notice {
        Notice { id: id.into(), text: text.into(), notify: true, critical: false }
    }
    pub fn critical(id: &str, text: impl Into<String>) -> Notice {
        Notice { id: id.into(), text: text.into(), notify: true, critical: true }
    }
}

/// The restart switches of `[restart]` in omarchyvrd.toml.
#[derive(Debug, Clone, PartialEq)]
pub struct Switches {
    /// Master switch: false = observe only (the default until the user turns it on).
    pub enabled: bool,
    /// P5: leaf and XR-client restarts.
    pub leaves: bool,
    /// P7: the Monado subtree, unplug/replug, resume.
    pub core: bool,
    /// P7: the automatic reset-headset after a camera-only drop (once, then ask).
    pub device_recover: bool,
    /// Hold systemd-inhibit while a session runs.
    pub inhibit_sleep: bool,
}

impl Default for Switches {
    fn default() -> Self {
        Switches { enabled: false, leaves: true, core: true, device_recover: true, inhibit_sleep: true }
    }
}

/// Everything the reconciler reads.
#[derive(Debug, Clone)]
pub struct World {
    pub comps: BTreeMap<Id, Comp>,
    pub desired: Desired,
    pub intent: Intent,
    pub budgets: Budgets,
    pub switches: Switches,
    pub gate: Gate,
    pub global: Global,
    /// The flag file or `safe-mode on`.
    pub safe_mode: bool,
    /// Entered by itself (global budget used up, a restart storm of omarchyvrd).
    pub safe_auto: Option<String>,
    pub facts: SessionFacts,
    /// No verdicts until then (after a resume).
    pub quiet_until: f64,
    /// One action in flight (its description) and its settle deadline.
    pub busy: Option<String>,
    pub settle_until: f64,
    /// The Monado process this generation belongs to (pid, start ticks).
    pub generation: Option<(u32, u64)>,
    /// Exit statuses of omarchyvrd's own children, by pid: (code, signal).
    pub exit_status: BTreeMap<u32, (Option<i32>, Option<i32>)>,
    /// Kernel crash lines by pid (time seen).
    pub crash_pids: BTreeMap<u32, f64>,
    /// Kernel `USB disconnect` times (correlation only).
    pub usb_disconnects: Vec<f64>,
    /// The camera was reset automatically this generation (once, then ask: user decision 2026-10-15).
    pub auto_reset_done: bool,
    /// The last WayVR Xid 31 (a WayVR crash with a GPU fault is not restarted automatically: the user is asked).
    pub wayvr_xid_at: Option<f64>,
    /// The last resume (frames frozen after a resume restart the session once).
    pub resumed_at: Option<f64>,
    pub now: f64,
    /// Pending notices for the caller to publish.
    pub notices: Vec<Notice>,
}

impl World {
    pub fn new(desired: Desired, budgets: Budgets, switches: Switches, gate: Gate, intent: Intent) -> World {
        let comps = SPECS.iter().map(|s| (s.id, Comp::default())).collect();
        World {
            comps,
            desired,
            intent,
            budgets,
            switches,
            gate,
            global: Global::default(),
            safe_mode: false,
            safe_auto: None,
            facts: SessionFacts::default(),
            quiet_until: 0.0,
            busy: None,
            settle_until: 0.0,
            generation: None,
            exit_status: BTreeMap::new(),
            crash_pids: BTreeMap::new(),
            usb_disconnects: Vec::new(),
            auto_reset_done: false,
            wayvr_xid_at: None,
            resumed_at: None,
            now: 0.0,
            notices: Vec::new(),
        }
    }

    pub fn c(&self, id: &str) -> &Comp {
        static EMPTY: std::sync::OnceLock<Comp> = std::sync::OnceLock::new();
        self.comps.get(id).unwrap_or_else(|| EMPTY.get_or_init(Comp::default))
    }

    fn cm(&mut self, id: &str) -> &mut Comp {
        let id = spec(id).map(|s| s.id).expect("known component");
        self.comps.entry(id).or_default()
    }

    /// Nothing automatic runs.
    pub fn observe_only(&self) -> bool {
        self.safe_mode || self.safe_auto.is_some() || !self.switches.enabled
    }

    /// The session mode in effect.
    pub fn mode(&self) -> Mode {
        self.facts.mode.unwrap_or(self.intent.mode)
    }

    /// The desired state with the session's mode.
    pub fn desired_now(&self) -> Desired {
        Desired { mode: self.mode(), ..self.desired.clone() }
    }

    /// Is `id` wanted right now?
    pub fn wanted(&self, id: &str) -> bool {
        let Some(s) = spec(id) else { return false };
        // Work's parts never run in a game session, whatever a toggle or an override says (Play = the whole GPU)
        if crate::spec::WORK_ONLY.contains(&s.id) && self.mode() == Mode::Vr {
            return false;
        }
        let d = self.desired_now();
        let ov = self.intent.overrides.get(id).copied();
        match s.scope {
            Scope::Always => (s.rule)(&d),
            Scope::Own => ov.unwrap_or((s.rule)(&d)),
            Scope::Session => {
                if !self.intent.on() {
                    return false;
                }
                if let Some(v) = ov {
                    return v;
                }
                match s.kind {
                    // omarchyvrd restores what this session had; it does not add what the launcher did not start
                    Kind::Process => self.c(id).seen_up || (self.owned_session() && (s.rule)(&d)),
                    // jobs run on demand (a relocalise after a start), never as a steady state
                    Kind::Job => false,
                    Kind::Device => match id {
                        "usb.cam" => self.wanted("broker") || self.wanted("passthrough"),
                        "usb.eye" => self.wanted("eye"),
                        _ => true,
                    },
                    _ => (s.rule)(&d) && s.deps.iter().filter(|x| x.hard && x.active(&d)).all(|x| spec(x.id).or_kind_wanted(Kind::Process, |_| self.wanted(x.id))),
                }
            }
        }
    }

    /// omarchyvrd itself started this session (P6: the launcher as a client); otherwise the session is adopted.
    pub fn owned_session(&self) -> bool {
        matches!(self.intent.by, By::Cli | By::User | By::Auto) && self.intent.overrides.contains_key("__owned")
    }

    /// A hard dependency of `id` (active for this configuration) that is down (debounced or raw) now or went down
    /// within the collateral window.
    fn dep_went_down(&self, id: &str, now: f64) -> Option<Id> {
        let d = self.desired_now();
        let s = spec(id)?;
        s.deps.iter().filter(|x| x.hard && x.active(&d)).map(|x| x.id).find(|dep| {
            let c = self.c(dep);
            c.raw == Some(false) || c.up == Some(false) && c.down_at.is_some_and(|t| now - t <= COLLATERAL_S) || self.dep_went_down(dep, now).is_some()
        })
    }

    /// Fold one round of observations. `log_crash(spec)`: the process log tail shows a crash (panic, abort...).
    pub fn observe(&mut self, now: f64, obs: &BTreeMap<Id, Obs>, facts: &SessionFacts, log_crash: &dyn Fn(&Spec) -> bool) {
        self.now = now;
        self.facts = facts.clone();
        for s in SPECS {
            if let Some(o) = obs.get(s.id) {
                self.observe_one(now, s, o, log_crash);
            }
        }
        // adoption: a Monado we have not seen
        let mo = self.c("monado").clone();
        if mo.up == Some(true) {
            let key = (mo.pid.unwrap_or(0), mo.start_ticks.unwrap_or(0));
            if self.generation != Some(key) {
                let first = self.generation.is_none();
                self.generation = Some(key);
                if !self.intent.on() && !facts.stopping {
                    // a session omarchyvrd did not start: adopt it (a new generation; the user's per-component
                    // overrides of non-session components such as ai.server are kept)
                    let keep: BTreeMap<String, bool> = self.intent.overrides.iter().filter(|(k, _)| spec(k).is_some_and(|s| s.scope != Scope::Session)).map(|(k, v)| (k.clone(), *v)).collect();
                    self.intent = Intent { session: Some(true), by: By::Adopted, mode: self.mode(), overrides: keep, at: crate::util::unix_now() };
                    self.notices.push(Notice::log("monado", format!("adopted the running {} session (pid {})", self.mode().name(), key.0)));
                    self.new_generation(false);
                } else if !first {
                    // restarted within the session (by omarchyvrd or the launcher): the restart history stays
                    self.notices.push(Notice::log("monado", format!("new Monado process (pid {})", key.0)));
                }
                // what runs now belongs to this session
                for s in SPECS.iter().filter(|s| s.kind == Kind::Process && s.scope == Scope::Session) {
                    if self.c(s.id).up == Some(true) {
                        self.cm(s.id).seen_up = true;
                    }
                }
            }
        }
        self.global.times.retain(|&t| now - t < 3600.0);
        let budgets = self.budgets.clone();
        for s in SPECS {
            let p = budgets.policy(s.class);
            self.cm(s.id).restart.tick(&p, now);
        }
        self.crash_pids.retain(|_, t| now - *t < 60.0);
    }

    /// A new session generation: forget what ran and every breaker (a session off / a new adopted Monado).
    pub fn new_generation(&mut self, keep_monado: bool) {
        for s in SPECS.iter().filter(|s| s.scope == Scope::Session) {
            if keep_monado && s.id == "monado" {
                continue;
            }
            let c = self.cm(s.id);
            c.seen_up = false;
            c.restart.reset();
            c.told.clear();
            c.last_exit = None;
        }
        self.auto_reset_done = false;
    }

    fn observe_one(&mut self, now: f64, s: &Spec, o: &Obs, log_crash: &dyn Fn(&Spec) -> bool) {
        let quiet = now < self.quiet_until;
        let (present_s, absent_s) = if s.kind == Kind::Device { (s.grace.present_s, s.grace.absent_s) } else { (0.0, 0.0) };
        let on = self.intent.on();
        let c = self.cm(s.id);
        if c.raw != o.up {
            c.raw = o.up;
            c.raw_since = now;
        }
        c.detail = o.detail.clone();
        c.degraded = o.degraded.clone();
        c.dstate = o.dstate;
        c.dstate_since = if o.dstate { c.dstate_since.or(Some(now)) } else { None };
        let stuck_d = s.id == "monado" && c.dstate_since.is_some_and(|t| now - t >= 10.0);
        if o.ready.is_some() {
            c.ready = o.ready;
        }
        let held = now - c.raw_since;
        let new_up = match (c.up, o.up) {
            (None, v) => v,
            (Some(prev), Some(v)) if prev != v => {
                let need = if v { present_s } else { absent_s };
                if held + 1e-9 >= need {
                    Some(v)
                } else {
                    Some(prev)
                }
            }
            (prev, None) => prev,
            (prev, _) => prev,
        };
        // a new process under the same id (restarted by someone): pid change while up
        let pid_changed = c.up == Some(true) && o.up == Some(true) && o.pid.is_some() && c.pid.is_some() && o.pid != c.pid;
        let was = c.up;
        let old_pid = c.pid;
        let up_for = now - c.since;
        if o.up == Some(true) {
            c.pid = o.pid;
            c.start_ticks = o.start_ticks;
        }
        if new_up != was {
            c.up = new_up;
            c.since = now;
        }
        if new_up == Some(true) {
            c.seen_up = c.seen_up || on;
            let c = self.cm(s.id);
            c.down_at = None;
            if c.ready != Some(false) || s.id != "monado" {
                c.starting_until = None;
            }
            if was == Some(false) && s.kind == Kind::Device {
                // new evidence: a replug resets the breakers tied to this device
                self.on_device_back(s.id);
            }
        }
        if stuck_d {
            self.raise_gate(GateLevel::NeedsReboot, "monado-service in D state (uninterruptible) for 10 s");
        }
        if pid_changed {
            self.notices.push(Notice::log(s.id, format!("{} restarted outside omarchyvrd (pid {} -> {})", s.id, old_pid.unwrap_or(0), o.pid.unwrap_or(0))));
            if s.id != "monado" {
                self.cm(s.id).restart.reset();
            }
        }
        if was == Some(true) && new_up == Some(false) {
            self.cm(s.id).down_at = Some(now);
            if s.kind == Kind::Process {
                self.on_process_down(now, s, old_pid, quiet, up_for, log_crash);
            } else if s.kind == Kind::Device {
                let detail = self.c(s.id).detail.clone();
                self.notices.push(Notice::log(s.id, format!("{} gone: {detail}", s.id)));
            }
        }
    }

    fn on_device_back(&mut self, dev: &str) {
        let d = self.desired_now();
        for id in crate::graph::dependents(dev, &d) {
            let c = self.cm(id);
            if matches!(c.restart.breaker, crate::policy::Breaker::Open { .. }) {
                c.restart.reset();
            }
        }
        if dev == "usb.cam" || dev == "usb.hid" {
            // a manual replug resets the device budget too
            let budgets_reset = self.c("usb.cam").restart.spent.len();
            if budgets_reset > 0 && dev == "usb.hid" {
                self.cm("usb.cam").restart.reset();
            }
        }
        self.notices.push(Notice::log(dev, format!("{dev} back")));
    }

    fn on_process_down(&mut self, now: f64, s: &Spec, pid: Option<u32>, quiet: bool, up_for: f64, log_crash: &dyn Fn(&Spec) -> bool) {
        let ours = self.c(s.id).stopping;
        let status = pid.and_then(|p| self.exit_status.remove(&p));
        let dep = self.dep_went_down(s.id, now);
        let crash_evidence = pid.is_some_and(|p| self.crash_pids.contains_key(&p)) || log_crash(s);
        let mut f = ExitFacts {
            code: status.and_then(|x| x.0),
            signal: status.and_then(|x| x.1),
            ours,
            dep_down: dep.is_some() || quiet,
            user_stop: self.facts.user_stop || self.facts.stopping,
            crash_evidence,
        };
        // Monado has no other stop path than the launcher (marker), omarchyvrd (ours) or a crash
        if s.id == "monado" && status.is_none() && !f.user_stop && !ours {
            f.crash_evidence = true;
        }
        let class = classify_exit(&f);
        let how = match (status, dep) {
            (_, Some(d)) => format!("after {d} went down"),
            (Some((Some(c), _)), _) => format!("exit {c}"),
            (Some((_, Some(sig))), _) => format!("signal {sig}"),
            _ if crash_evidence => "crash evidence in the logs".into(),
            _ => "no exit status (adopted)".into(),
        };
        let text = format!("{} exited: {} ({how})", s.id, class.name());
        {
            let c = self.cm(s.id);
            c.stopping = false;
            c.starting_until = None;
            c.last_exit = Some((now, class, how.clone()));
        }
        let p = self.budgets.policy(s.class);
        match class {
            ExitClass::Crash => {
                let opened = self.cm(s.id).restart.crashed_after(&p, now, up_for);
                let critical = s.class == Class::Core;
                self.notices.push(if critical { Notice::critical(s.id, &text) } else { Notice::log(s.id, &text) });
                if opened {
                    let why = match &self.c(s.id).restart.breaker {
                        crate::policy::Breaker::Open { why, .. } => why.clone(),
                        _ => String::new(),
                    };
                    self.notices.push(Notice::critical(s.id, format!("{} keeps failing ({why}): not restarted again; Fix in the app", s.label)));
                }
            }
            ExitClass::User => {
                self.notices.push(Notice::log(s.id, &text));
                if s.id == "monado" {
                    self.set_intent_off(By::User, false);
                } else if self.intent.on() && s.scope == Scope::Session && !self.facts.stopping && !self.facts.user_stop {
                    self.intent.overrides.insert(s.id.to_string(), false);
                    self.notices.push(Notice::tell(s.id, format!("{} was closed outside omarchyvrd: turned off for this session", s.label)));
                } else if s.scope == Scope::Own {
                    self.intent.overrides.insert(s.id.to_string(), false);
                }
            }
            ExitClass::Unknown => {
                self.notices.push(Notice::log(s.id, &text));
            }
            ExitClass::Expected | ExitClass::Collateral => self.notices.push(Notice::log(s.id, &text)),
        }
    }

    /// The session should not run. `active`: omarchyvrd should stop what still runs (a client asked); a launcher stop
    /// already stops everything itself.
    pub fn set_intent_off(&mut self, by: By, active: bool) {
        let mode = self.mode();
        self.intent = Intent { session: Some(false), by, mode, overrides: BTreeMap::new(), at: crate::util::unix_now() };
        if active {
            self.intent.overrides.insert("__stop".into(), true);
        }
        self.new_generation(false);
        self.generation = self.generation.filter(|_| active);
    }

    /// A kernel log line: the gate, crash pids, USB disconnects. Returns true when the gate went up.
    pub fn kernel_line(&mut self, now: f64, line: &str) -> bool {
        if let Some((_, pid)) = crate::gpu::crash_line(line) {
            self.crash_pids.insert(pid, now);
        }
        if crate::gpu::usb_disconnect_line(line).is_some() {
            self.usb_disconnects.retain(|&t| now - t < 600.0);
            self.usb_disconnects.push(now);
        }
        if let Some(crate::gpu::GpuEvent::Xid { code: 31, name: Some(n), .. }) = crate::gpu::kernel_line(line) {
            if n == "wayvr" {
                self.wayvr_xid_at = Some(now);
            }
        }
        match crate::gpu::kernel_line(line) {
            Some(ev) => self.raise_gate(ev.level(), &format!("kernel: {}", line.trim())),
            None => false,
        }
    }

    /// A Monado log line.
    pub fn monado_line(&mut self, line: &str) -> bool {
        match crate::gpu::monado_line(line) {
            Some(ev) => self.raise_gate(ev.level(), &format!("monado: {}", line.trim())),
            None => false,
        }
    }

    pub fn raise_gate(&mut self, level: GateLevel, evidence: &str) -> bool {
        let up = self.gate.raise(level, evidence);
        if up {
            let n = match self.gate.effective() {
                GateLevel::NeedsReboot => Notice::critical(
                    "gpu.gate",
                    format!("Reboot needed (known NVIDIA issue): no VR start until a reboot. Evidence: {evidence}"),
                ),
                _ => Notice::tell("gpu.gate", format!("GPU warning (WayVR fault, warning only): {evidence}")),
            };
            self.notices.push(n);
        }
        up
    }

    /// The daemon is about to execute `st` (pure bookkeeping: budgets, busy, expected exits, starting deadlines).
    /// Notices only mark themselves told. Returns false when the step must not run after all (the global budget
    /// just ran out: safe mode).
    pub fn begin(&mut self, st: &crate::reconcile::Step, now: f64) -> bool {
        use crate::reconcile::{Act, SUBTREE};
        if let Act::Notify { key, .. } = &st.act {
            if let Some(c) = self.comps.get_mut(st.id) {
                c.told.push(key.clone());
            }
            return true;
        }
        if !st.act.acts() {
            return true;
        }
        if !self.global.allows(self.budgets.global, now) {
            self.safe_auto = Some(format!("global budget {} used up", crate::policy::budget_text(self.budgets.global)));
            self.notices.push(Notice::critical("omarchyvrd", "Too many automatic repairs: omarchyvrd entered safe mode (nothing runs automatically until you turn it off)"));
            return false;
        }
        self.global.spend(self.budgets.global, now);
        if st.spend {
            if let Some(c) = self.comps.get_mut(st.id) {
                c.restart.spend(now);
            }
        }
        self.busy = Some(st.line());
        let up_subtree: Vec<Id> = SUBTREE.iter().copied().chain(["monado"]).filter(|id| self.c(id).up == Some(true)).collect();
        match st.act {
            Act::Start => {
                if st.id == "monado" {
                    self.intent.overrides.remove("__force"); // a forced start passes once
                }
                let g = spec(st.id).map(|s| s.grace.start_s).unwrap_or(10.0);
                self.cm(st.id).starting_until = Some(now + g + 30.0);
                if st.id == "monado" {
                    for id in SUBTREE {
                        if self.c(id).seen_up && self.wanted(id) {
                            self.cm(id).starting_until = Some(now + 120.0);
                        }
                    }
                }
            }
            Act::Stop => self.cm(st.id).stopping = true,
            Act::StopSubtree | Act::ResetHeadset => {
                for id in up_subtree {
                    self.cm(id).stopping = true;
                }
                if st.act == Act::ResetHeadset {
                    self.auto_reset_done = true;
                }
            }
            _ => {}
        }
        true
    }

    /// The executor finished `st`.
    pub fn finish(&mut self, st: &crate::reconcile::Step, outcome: &Outcome, now: f64) {
        use crate::reconcile::Act;
        self.busy = None;
        let settle = spec(st.id).map(|s| s.grace.settle_s).unwrap_or(3.0).max(2.0);
        self.settle_until = now + settle;
        match outcome {
            Outcome::Ok => {
                self.notices.push(Notice::log(st.id, format!("done: {}", st.line())));
                if st.act == Act::Start && st.id == "wayvr" && self.wanted("preset") {
                    self.cm("wayvr").told.push("reapply-preset".into());
                }
                if st.act == Act::Reapply {
                    self.cm("wayvr").told.retain(|k| k != "reapply-preset");
                }
            }
            Outcome::Failed(why) => {
                self.notices.push(Notice::tell(st.id, format!("{} failed: {why}", st.line())));
                if st.act == Act::Start {
                    let p = self.budgets.policy(spec(st.id).map(|s| s.class).unwrap_or(crate::model::Class::None));
                    let c = self.cm(st.id);
                    c.starting_until = None;
                    c.down_at = Some(now);
                    c.last_exit = Some((now, ExitClass::Crash, format!("start failed: {why}")));
                    c.restart.crashed(&p, now);
                }
                for c in self.comps.values_mut() {
                    c.stopping = false;
                    if st.act == Act::Start && st.id == "monado" {
                        c.starting_until = None;
                    }
                }
            }
            Outcome::Hung(why) => {
                // never SIGKILL Monado automatically: the display would likely need a reboot anyway
                self.raise_gate(GateLevel::NeedsReboot, &format!("{} still alive after its stop grace: {why}", st.id));
                if st.id == "monado" || matches!(st.act, crate::reconcile::Act::StopSubtree | crate::reconcile::Act::ResetHeadset) {
                    self.notices.push(Notice::critical(
                        "monado",
                        "The VR runtime hangs and was NOT killed. Choose: kill it yourself (the display will likely need a reboot), or reboot.",
                    ));
                }
                for c in self.comps.values_mut() {
                    c.stopping = false;
                }
            }
        }
    }

    /// The machine resumed from suspend: probes are stale, no verdicts for `grace_s`.
    pub fn resumed(&mut self, now: f64, jump_s: f64, grace_s: f64) {
        self.quiet_until = now + grace_s;
        self.resumed_at = Some(now);
        self.notices.push(Notice::log("omarchyvrd", format!("resumed from suspend ({jump_s:.0} s asleep): {grace_s:.0} s grace")));
    }

    /// Health of every component (topological order, so dependencies are known first).
    pub fn health(&self) -> BTreeMap<Id, Health> {
        let now = self.now;
        let d = self.desired_now();
        let mut h: BTreeMap<Id, Health> = BTreeMap::new();
        for id in crate::graph::start_order() {
            let s = spec(id).expect("table");
            let c = self.c(id);
            let w = self.wanted(id);
            let bad_dep = s
                .deps
                .iter()
                .filter(|x| x.hard && x.active(&d))
                .find(|x| {
                    let hh = h.get(x.id);
                    // Monado is not blocked by a client process that is merely not running: the launcher start
                    // brings the broker up first
                    let launcher_dep = id == "monado" && c.up != Some(true) && spec(x.id).is_some_and(|d| d.kind == Kind::Process) && matches!(hh, Some(Health::Down(_)));
                    !launcher_dep && !hh.is_some_and(|hh| hh.usable() || (*hh == Health::Off && !self.wanted(x.id) && s.kind != Kind::Process))
                })
                .map(|x| x.id);
            let v = if s.kind == Kind::Gate {
                match self.gate.effective() {
                    GateLevel::Ok if self.gate.level == GateLevel::Caution => Health::Ok,
                    GateLevel::Ok => Health::Ok,
                    GateLevel::Caution => Health::Degraded(self.gate.evidence.last().cloned().unwrap_or_else(|| "WayVR Xid 31".into())),
                    GateLevel::NeedsReboot => Health::Down(format!("reboot needed: {}", self.gate.evidence.last().cloned().unwrap_or_default())),
                }
            } else if !w {
                match c.up {
                    Some(true) if s.kind == Kind::Process => Health::Ok,
                    _ => Health::Off,
                }
            } else {
                match c.up {
                    Some(true) => {
                        if let Some(dg) = &c.degraded {
                            Health::Degraded(dg.clone())
                        } else if let Some(dep) = bad_dep {
                            Health::Degraded(format!("{dep} is not up"))
                        } else if c.starting_until.is_some_and(|t| now < t) || (id == "monado" && c.ready == Some(false)) {
                            Health::Starting { until: c.starting_until.unwrap_or(c.since + s.grace.start_s) }
                        } else {
                            Health::Ok
                        }
                    }
                    Some(false) | None => {
                        if let crate::policy::Breaker::Open { why, .. } = &c.restart.breaker {
                            Health::Failed(why.clone())
                        } else if let Some(dep) = bad_dep {
                            Health::Blocked(dep.to_string())
                        } else if c.starting_until.is_some_and(|t| now < t) {
                            Health::Starting { until: c.starting_until.unwrap_or(now) }
                        } else if c.up.is_none() {
                            Health::Unknown
                        } else {
                            let why = match &c.last_exit {
                                Some((t, cl, how)) => format!("exited ({}, {how}) {:.0} s ago", cl.name(), now - t),
                                None => c.detail.clone(),
                            };
                            Health::Down(why)
                        }
                    }
                }
            };
            h.insert(id, v);
        }
        h
    }

    /// The one-word summary.
    pub fn overall(&self, h: &BTreeMap<Id, Health>) -> Overall {
        if self.gate.effective() == GateLevel::NeedsReboot {
            return Overall::NeedsReboot;
        }
        let mut o = Overall::Ok;
        for (id, hh) in h {
            let s = spec(id).expect("table");
            if !self.wanted(id) {
                continue;
            }
            let core = matches!(*id, "monado" | "usb.hid" | "display.dp1");
            let v = match hh {
                Health::Ok | Health::Off | Health::Starting { .. } => Overall::Ok,
                Health::Failed(_) => Overall::NeedsYou,
                Health::Down(_) | Health::Blocked(_) if core => Overall::Down,
                Health::Down(_) | Health::Blocked(_) | Health::Degraded(_) | Health::Unknown => {
                    if s.kind == Kind::Setting || s.kind == Kind::Gate || *hh == Health::Unknown {
                        // host settings warn (the GUI shows them), unknown is not a fault
                        if (matches!(hh, Health::Down(_)) && s.kind == Kind::Setting) || matches!(hh, Health::Degraded(_)) {
                            Overall::Degraded
                        } else {
                            Overall::Ok
                        }
                    } else {
                        Overall::Degraded
                    }
                }
            };
            o = o.max(v);
        }
        if o < Overall::NeedsYou && self.safe_auto.is_some() {
            o = Overall::NeedsYou;
        }
        o
    }
}

trait SpecKindExt {
    fn or_kind_wanted(self, k: Kind, f: impl Fn(&Spec) -> bool) -> bool;
}

impl SpecKindExt for Option<&'static Spec> {
    /// Non-process deps do not gate wanted-ness; process deps must be wanted themselves.
    fn or_kind_wanted(self, k: Kind, f: impl Fn(&Spec) -> bool) -> bool {
        match self {
            Some(s) if s.kind == k => f(s),
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::Obs;

    pub fn obs_all(up: &[(&'static str, Option<bool>)]) -> BTreeMap<Id, Obs> {
        let mut m: BTreeMap<Id, Obs> = SPECS.iter().map(|s| (s.id, Obs { up: Some(false), ..Default::default() })).collect();
        for &(id, v) in up {
            let pid = spec(id).and_then(|s| s.pid).map(|_| 1000 + id.len() as u32);
            m.insert(id, Obs { up: v, pid: if v == Some(true) { pid } else { None }, start_ticks: Some(1), ready: Some(true), ..Default::default() });
        }
        m
    }

    pub fn desk_up() -> Vec<(&'static str, Option<bool>)> {
        ["usb.hid", "usb.cam", "usb.eye", "keepalive", "display.dp1", "gpu.gate", "host.caps", "host.mhs", "host.runtime-link", "broker", "monado", "eye", "sixdof", "wayvr", "passthrough", "ctrltrack", "perf", "eye.laser", "preset"]
            .iter()
            .map(|id| (*id, Some(true)))
            .collect()
    }

    fn world() -> World {
        World::new(Desired::default(), Budgets::default(), Switches { enabled: true, ..Default::default() }, Gate::new("b"), Intent::default())
    }

    fn facts_desk() -> SessionFacts {
        SessionFacts { mode: Some(Mode::Desk), ..Default::default() }
    }

    #[test]
    fn adopts_a_running_desk_session() {
        let mut w = world();
        w.observe(0.0, &obs_all(&desk_up()), &facts_desk(), &|_| false);
        assert!(w.intent.on());
        assert_eq!(w.intent.by, By::Adopted);
        let h = w.health();
        assert_eq!(h["monado"], Health::Ok);
        assert_eq!(h["wayvr"], Health::Ok);
        assert_eq!(h["voice.listen"], Health::Off, "not running and not started by the launcher");
        assert_eq!(w.overall(&h), Overall::Ok);
        assert!(w.wanted("perf") && !w.wanted("voice.listen"));
    }

    #[test]
    fn a_camera_drop_blocks_its_dependents_and_degrades() {
        let mut w = world();
        w.observe(0.0, &obs_all(&desk_up()), &facts_desk(), &|_| false);
        let mut up = desk_up();
        for e in up.iter_mut() {
            if matches!(e.0, "usb.cam" | "broker" | "passthrough" | "ctrltrack") {
                e.1 = Some(false);
            }
        }
        w.observe(1.0, &obs_all(&up), &facts_desk(), &|_| false);
        // the device is debounced (20 s for the camera); the processes are gone at once and are collateral
        assert_eq!(w.c("usb.cam").up, Some(true));
        assert_eq!(w.c("broker").last_exit.as_ref().unwrap().1, ExitClass::Collateral);
        w.observe(22.0, &obs_all(&up), &facts_desk(), &|_| false);
        let h = w.health();
        assert!(matches!(h["usb.cam"], Health::Down(_)), "{:?}", h["usb.cam"]);
        assert_eq!(h["broker"], Health::Blocked("usb.cam".into()));
        assert!(matches!(&h["passthrough"], Health::Blocked(d) if d == "broker" || d == "usb.cam"), "{:?}", h["passthrough"]);
        assert!(matches!(h["monado"], Health::Degraded(_)), "{:?}", h["monado"]);
        assert_eq!(w.overall(&h), Overall::Degraded);
        assert_eq!(w.c("broker").restart.spent.len(), 0, "no budget spent");
    }

    #[test]
    fn a_user_close_turns_the_component_off_and_a_crash_does_not() {
        let mut w = world();
        let mut up = desk_up();
        up.push(("voice.listen", Some(true)));
        w.observe(0.0, &obs_all(&up), &facts_desk(), &|_| false);
        assert!(w.wanted("voice.listen"));
        // perf crashes (log shows a panic), voice exits without evidence: unknown, wayvr exits 0 (our child)
        let wayvr_pid = w.c("wayvr").pid.unwrap();
        w.exit_status.insert(wayvr_pid, (Some(0), None));
        let mut up2 = up.clone();
        for e in up2.iter_mut() {
            if matches!(e.0, "perf" | "voice.listen" | "wayvr") {
                e.1 = Some(false);
            }
        }
        // wayvr exits first: perf (depends on wayvr) then is collateral
        w.observe(5.0, &obs_all(&up2), &facts_desk(), &|s| s.id == "perf");
        assert_eq!(w.c("wayvr").last_exit.as_ref().unwrap().1, ExitClass::User);
        assert_eq!(w.intent.overrides.get("wayvr"), Some(&false));
        assert!(!w.wanted("wayvr"));
        assert_eq!(w.c("perf").last_exit.as_ref().unwrap().1, ExitClass::Collateral);
        assert_eq!(w.c("voice.listen").last_exit.as_ref().unwrap().1, ExitClass::Unknown);
        assert!(w.notices.iter().any(|n| n.notify && n.text.contains("closed outside omarchyvrd")));
    }

    #[test]
    fn a_launcher_stop_sets_the_intent_off() {
        let mut w = world();
        w.observe(0.0, &obs_all(&desk_up()), &facts_desk(), &|_| false);
        let down: Vec<_> = desk_up().into_iter().map(|(id, v)| if spec(id).unwrap().kind == Kind::Process && id != "keepalive" { (id, Some(false)) } else { (id, v) }).collect();
        let f = SessionFacts { user_stop: true, ..facts_desk() };
        w.observe(3.0, &obs_all(&down), &f, &|_| false);
        assert_eq!(w.intent.session, Some(false));
        assert_eq!(w.intent.by, By::User);
        let h = w.health();
        assert_eq!(h["monado"], Health::Off);
        assert_eq!(w.overall(&h), Overall::Ok);
    }

    #[test]
    fn monado_without_a_stop_marker_is_a_crash_and_its_clients_are_collateral() {
        let mut w = world();
        w.observe(0.0, &obs_all(&desk_up()), &facts_desk(), &|_| false);
        let down: Vec<_> = desk_up()
            .into_iter()
            .map(|(id, v)| if matches!(id, "monado" | "wayvr" | "passthrough" | "ctrltrack" | "perf") { (id, Some(false)) } else { (id, v) })
            .collect();
        w.observe(2.0, &obs_all(&down), &facts_desk(), &|_| false);
        assert_eq!(w.c("monado").last_exit.as_ref().unwrap().1, ExitClass::Crash);
        assert_eq!(w.c("wayvr").last_exit.as_ref().unwrap().1, ExitClass::Collateral);
        assert!(w.intent.on(), "a crash keeps the intent on");
        let h = w.health();
        assert!(matches!(h["monado"], Health::Down(_)));
        assert_eq!(h["wayvr"], Health::Blocked("monado".into()));
        assert_eq!(w.overall(&h), Overall::Down);
        assert!(w.notices.iter().any(|n| n.critical && n.id == "monado"));
    }

    #[test]
    fn monado_stuck_in_d_state_latches_needs_reboot() {
        let mut w = world();
        let mut o = obs_all(&desk_up());
        w.observe(0.0, &o, &facts_desk(), &|_| false);
        o.get_mut("monado").unwrap().dstate = true;
        w.observe(1.0, &o, &facts_desk(), &|_| false);
        w.observe(5.0, &o, &facts_desk(), &|_| false);
        assert_eq!(w.gate.effective(), GateLevel::Ok, "a short D state is normal");
        w.observe(11.0, &o, &facts_desk(), &|_| false);
        assert_eq!(w.gate.effective(), GateLevel::NeedsReboot);
    }

    #[test]
    fn the_gate_drives_overall() {
        let mut w = world();
        w.observe(0.0, &obs_all(&desk_up()), &facts_desk(), &|_| false);
        assert!(w.kernel_line(1.0, "NVRM: Xid (PCI:0000:01:00): 31, pid=5, name=wayvr, channel 1"));
        let h = w.health();
        assert!(matches!(h["gpu.gate"], Health::Degraded(_)));
        assert_eq!(w.overall(&h), Overall::Degraded);
        assert!(w.kernel_line(2.0, "nvidia-modeset: ERROR: GPU:0: Idling display engine timed out"));
        assert_eq!(w.overall(&w.health()), Overall::NeedsReboot);
        assert!(!w.kernel_line(3.0, "usb 2-4: USB disconnect, device number 7"));
        assert_eq!(w.usb_disconnects.len(), 1);
    }
}
