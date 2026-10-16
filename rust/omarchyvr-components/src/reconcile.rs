//! The pure `reconcile()` (plan section 3.5): desired state + intent + actual state + policy + gate -> an ordered
//! plan. The daemon executes at most the first runnable step (one action in flight, each with a settle time) and
//! publishes the notices; in safe mode / observe-only it only logs the plan as "would".
//!
//! Rules:
//! * The user's intent wins: nothing starts when the session is off; a component the user closed stays off; stops
//!   happen only when a client asked (session off or a toggle off).
//! * Top-down start: a component starts only when its hard dependencies are up. A blocked component waits for
//!   the device and spends no budget. Only crashes spend budget; collateral and omarchyvrd's own stops are free.
//! * Monado: in desk mode an ordered subtree restart through the launcher (SIGINT only, never SIGKILL); in a game
//!   (vr mode) only a notice: a game is never stopped or restarted. Deferred to the launcher's watchdog while one
//!   runs. Never while the GPU gate says needs-reboot.
//! * Headset unplugged: desk = a clean ordered stop, restored on replug; game = notify and wait (user decision 4).
//! * Camera-only drop (HID present) for 20 s: desk = reset-headset once automatically, then ask (decision 5).
use crate::model::{Class, GateLevel, Health, Id, Kind, Level, Mode};
use crate::policy::{ExitClass, Verdict};
use crate::spec::{spec, Scope};
use crate::state::World;

/// What to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    /// Nothing until `until` (or an event).
    Wait { until: Option<f64> },
    /// Tell the user (once per key and session).
    Notify { key: String, critical: bool },
    /// Start a component that is down. Monado: the launcher start with the recorded arguments (the whole subtree).
    Start,
    /// Ordered stop of the Monado subtree (clients, WayVR exited, Monado SIGINT, broker), then let `Start` restore.
    StopSubtree,
    /// Ordered subtree stop, then `omarchyvr vr reset-headset` (approved exit bytes), then let `Start` restore.
    ResetHeadset,
    /// Stop one component (a toggle off or a session off from a client).
    Stop,
    /// L1: re-apply a live setting (the preset after a WayVR restart).
    Reapply,
}

impl Act {
    pub fn name(&self) -> &'static str {
        match self {
            Act::Wait { .. } => "wait",
            Act::Notify { .. } => "notify",
            Act::Start => "start",
            Act::StopSubtree => "stop-subtree",
            Act::ResetHeadset => "reset-headset",
            Act::Stop => "stop",
            Act::Reapply => "reapply",
        }
    }
    /// Does something to processes or devices (not a wait or a notice).
    pub fn acts(&self) -> bool {
        !matches!(self, Act::Wait { .. } | Act::Notify { .. })
    }
}

/// One step.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub id: Id,
    pub act: Act,
    pub level: Level,
    pub why: String,
    /// Spends restart budget (a crash restart, the automatic reset-headset).
    pub spend: bool,
}

impl Step {
    fn new(id: Id, act: Act, level: Level, why: impl Into<String>) -> Step {
        Step { id, act, level, why: why.into(), spend: false }
    }
    pub fn line(&self) -> String {
        format!("{} {} ({:?}): {}", self.act.name(), self.id, self.level, self.why)
    }
}

/// The plan, in order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Plan {
    pub steps: Vec<Step>,
}

impl Plan {
    /// The first step that does something.
    pub fn action(&self) -> Option<&Step> {
        self.steps.iter().find(|s| s.act.acts())
    }
    pub fn notices(&self) -> impl Iterator<Item = &Step> {
        self.steps.iter().filter(|s| matches!(s.act, Act::Notify { .. }))
    }
    /// The step shown as `next_action` in the snapshot: the action, else the first wait.
    pub fn next(&self) -> Option<&Step> {
        self.action().or_else(|| self.steps.iter().find(|s| matches!(s.act, Act::Wait { .. })))
    }
}

/// Processes the launcher's `desk|vr start` brings up together with Monado.
pub const SUBTREE: &[Id] = &["broker", "wayvr", "passthrough", "ctrltrack", "perf", "screens", "voice.listen", "voice.captions", "voice.act", "voicehud", "floor.follow"];

/// Seconds the headset must be missing before the L5 notice.
pub const HID_TELL_S: f64 = 30.0;

pub fn reconcile(w: &World) -> Plan {
    let now = w.now;
    let h = w.health();
    let mut p = Plan::default();
    let told = |id: &str, key: &str| w.c(id).told.iter().any(|k| k == key);
    let notify = |p: &mut Plan, id: Id, key: &str, critical: bool, level: Level, text: String| {
        if !told(id, key) {
            p.steps.push(Step::new(id, Act::Notify { key: key.into(), critical }, level, text));
        }
    };
    let mode = w.mode();
    let up = |id: &str| w.c(id).up == Some(true);

    // after needs-reboot nothing is stopped or restarted around Monado (a hung one stays as it is: never killed)
    let reboot = w.gate.effective() == GateLevel::NeedsReboot;
    if now < w.quiet_until {
        p.steps.push(Step::new("omarchyvrd", Act::Wait { until: Some(w.quiet_until) }, Level::L0, "resumed from suspend: no verdicts yet"));
        return p;
    }

    // a client asked for the session off: ordered stop of what still runs (leaves first, the broker last)
    if w.intent.session == Some(false) && w.intent.overrides.contains_key("__stop") {
        if let Some(id) = crate::graph::stop_order().into_iter().find(|id| {
            let s = spec(id).unwrap();
            s.kind == Kind::Process && s.scope == Scope::Session && up(id) && !w.c(id).stopping
        }) {
            p.steps.push(Step::new(id, Act::Stop, Level::L2, "session off"));
        }
        return p;
    }
    // explicit toggles off of running components
    for id in crate::graph::stop_order() {
        let s = spec(id).unwrap();
        if s.kind == Kind::Process && up(id) && w.intent.overrides.get(id) == Some(&false) && !w.c(id).stopping {
            p.steps.push(Step::new(id, Act::Stop, Level::L2, "toggled off"));
        }
    }

    // ai.server: Work sessions only (its rule); planned here like before, so a session switch to Play stops it
    plan_process(w, &h, "ai.server", &mut p);

    if !w.intent.on() {
        return p;
    }
    if w.facts.starting || w.facts.stopping {
        p.steps.push(Step::new("monado", Act::Wait { until: None }, Level::L0, if w.facts.starting { "the launcher is starting the session" } else { "the launcher is stopping the session" }));
        return p;
    }

    // the headset
    if !h["usb.hid"].usable() && w.wanted("usb.hid") {
        let gone_s = now - w.c("usb.hid").since;
        let key = "hid-gone";
        match mode {
            Mode::Desk if up("monado") && w.switches.core && !reboot => {
                p.steps.push(Step::new("monado", Act::StopSubtree, Level::L3, "headset unplugged: clean ordered stop, restored on replug"));
            }
            Mode::Desk => notify(&mut p, "usb.hid", key, false, Level::L5, "Headset disconnected: the desk session resumes when it is back".into()),
            Mode::Vr => notify(&mut p, "usb.hid", key, false, Level::L5, "Headset disconnected: waiting for it (the game keeps running)".into()),
        }
        if gone_s >= HID_TELL_S {
            notify(&mut p, "usb.hid", "hid-30s", false, Level::L5, "Headset not connected for 30 s: check the cable and the link box power".into());
        }
        p.steps.push(Step::new("usb.hid", Act::Wait { until: None }, Level::L0, "waiting for the headset (34a4:0044) to come back"));
        return p;
    }
    if w.c("usb.hid").raw == Some(false) {
        p.steps.push(Step::new("usb.hid", Act::Wait { until: Some(w.c("usb.hid").raw_since + 6.0) }, Level::L0, "headset gone: 6 s debounce"));
        return p;
    }

    // the camera (HID present)
    if w.wanted("usb.cam") {
        let cam = w.c("usb.cam");
        if matches!(h["usb.cam"], Health::Down(_)) && w.busy.is_some() {
            p.steps.push(Step::new("usb.cam", Act::Wait { until: None }, Level::L4, "headset restart running (up to 150 s)"));
        } else if matches!(h["usb.cam"], Health::Down(_)) {
            let pol = w.budgets.policy(Class::Device);
            let budget_ok = matches!(cam.restart.verdict(&pol, now, cam.since), Verdict::Now);
            match mode {
                Mode::Desk if w.switches.device_recover && !w.auto_reset_done && budget_ok => {
                    let mut s = Step::new("usb.cam", Act::ResetHeadset, Level::L4, "camera gone for 20 s with the headset present: reset-headset once");
                    s.spend = true;
                    notify(&mut p, "usb.cam", "cam-drop", false, Level::L5, "Camera dropped: restarting the headset (about 2 min)".into());
                    p.steps.push(s);
                }
                Mode::Desk => notify(&mut p, "usb.cam", "cam-ask", true, Level::L5, "Camera did not come back. Try Restart headset once more, or reboot.".into()),
                Mode::Vr => notify(&mut p, "usb.cam", "cam-ask", false, Level::L5, "Camera dropped: 6DoF and passthrough paused. Restart headset when the game allows it.".into()),
            }
        } else if cam.raw == Some(false) && cam.up == Some(true) {
            notify(&mut p, "usb.cam", "cam-blip", false, Level::L0, "Camera dropped: 6DoF and passthrough paused, the desktop still works".into());
            p.steps.push(Step::new("usb.cam", Act::Wait { until: Some(cam.raw_since + 20.0) }, Level::L0, "waiting 20 s for the camera to return by itself"));
        }
    }
    if !h["display.dp1"].usable() && w.wanted("display.dp1") && h["usb.hid"].usable() {
        p.steps.push(Step::new("display.dp1", Act::Wait { until: None }, Level::L0, "waiting for the headset display (drm uevent); never a hotplug"));
        if now - w.c("display.dp1").since > 30.0 {
            notify(&mut p, "display.dp1", "dp-30s", false, Level::L5, "Headset display not connected for 30 s".into());
        }
    }

    // after a resume: Monado alive but frames frozen
    let after_resume = w.resumed_at.is_some_and(|r| now - r < 120.0);
    if after_resume && up("monado") && w.c("monado").degraded.as_deref().is_some_and(|d| d.starts_with("frames stalled")) {
        if mode == Mode::Desk && w.switches.core && !w.facts.watchdog && !reboot {
            p.steps.push(Step::new("monado", Act::StopSubtree, Level::L3, "frames not advancing: restart the session"));
        } else {
            notify(&mut p, "monado", "stalled", false, Level::L5, "VR frames are not advancing: restart the session".into());
        }
    }

    // the broker came back without SLAM reconnecting: Monado's subtree must restart (until P9)
    let cam_steady = w.c("usb.cam").raw == Some(true) && h["usb.cam"].usable();
    if !reboot && mode == Mode::Desk && up("monado") && w.desired_now().sixdof && w.wanted("broker") && !up("broker") && cam_steady && !w.c("broker").restart.breaker_open() {
        p.steps.push(Step::new("broker", Act::StopSubtree, Level::L3, "camera back: the broker and Monado's subtree restart (SLAM cannot reconnect yet)"));
    }

    let monado_down = w.wanted("monado") && !up("monado");
    if monado_down && reboot && w.c("monado").last_exit.as_ref().not_user() {
        if w.intent.overrides.contains_key("__force") {
            p.steps.push(Step::new("monado", Act::Start, Level::L3, "forced start past needs-reboot (the user's choice)"));
        } else {
            notify(&mut p, "monado", "gate", true, Level::L5, "VR runtime not restarted: the GPU needs a reboot".into());
        }
    }
    for id in crate::graph::start_order() {
        let s = spec(id).unwrap();
        if s.kind != Kind::Process || s.scope != Scope::Session {
            continue;
        }
        if monado_down && SUBTREE.contains(&id) {
            continue; // the launcher start brings them up with Monado
        }
        if reboot && SUBTREE.contains(&id) && id != "voice.listen" {
            // the session's subtree waits for the reboot; voice on the CPU keeps going
            if w.wanted(id) && matches!(h[id], Health::Down(_)) {
                notify(&mut p, id, "gate", true, Level::L5, format!("{} not restarted: the GPU needs a reboot", s.label));
            }
            continue;
        }
        plan_process(w, &h, id, &mut p);
    }
    // L1: the preset after WayVR came back under omarchyvrd
    if w.wanted("preset") && up("wayvr") && w.c("wayvr").starting_until.is_none() && w.c("wayvr").told.iter().any(|k| k == "reapply-preset") {
        p.steps.push(Step::new("preset", Act::Reapply, Level::L1, "re-apply the preset after the WayVR restart"));
    }
    p
}

/// A device in the hard-dependency closure of `id` that is missing (raw or debounced).
pub fn missing_device(w: &World, id: &str, depth: usize) -> Option<Id> {
    let d = w.desired_now();
    let s = spec(id)?;
    if depth > 8 {
        return None;
    }
    for x in s.deps.iter().filter(|x| x.hard && x.active(&d)) {
        let ds = spec(x.id)?;
        if ds.kind == Kind::Device && (w.c(x.id).raw == Some(false) || w.c(x.id).up == Some(false)) {
            return Some(ds.id);
        }
        if let Some(m) = missing_device(w, x.id, depth + 1) {
            return Some(m);
        }
    }
    None
}

trait LastExitExt {
    fn not_user(self) -> bool;
}

impl LastExitExt for Option<&(f64, ExitClass, String)> {
    fn not_user(self) -> bool {
        !matches!(self, Some((_, ExitClass::User, _)))
    }
}

impl crate::policy::RestartState {
    pub fn breaker_open(&self) -> bool {
        matches!(self.breaker, crate::policy::Breaker::Open { .. })
    }
}

fn plan_process(w: &World, h: &std::collections::BTreeMap<Id, Health>, id: Id, p: &mut Plan) {
    let s = spec(id).unwrap();
    let c = w.c(id);
    if !w.wanted(id) || !matches!(h[id], Health::Down(_)) {
        return;
    }
    let told = |key: &str| c.told.iter().any(|k| k == key);
    let push_note = |p: &mut Plan, key: &str, critical: bool, text: String| {
        if !told(key) {
            p.steps.push(Step::new(id, Act::Notify { key: key.into(), critical }, Level::L5, text));
        }
    };
    if let Some(dev) = missing_device(w, id, 0) {
        let _ = dev;
        return; // a device it needs is (still) missing: wait for it, no restart, no budget
    }
    let class = c.last_exit.as_ref().map(|e| e.1);
    match class {
        Some(ExitClass::Unknown) => {
            push_note(p, "unknown-exit", false, format!("{} exited with no crash evidence: not restarted (Fix restarts it)", s.label));
            return;
        }
        Some(ExitClass::User) => return,
        None if !w.owned_session() && s.scope == Scope::Session && w.intent.overrides.get(id) != Some(&true) => return,
        _ => {}
    }
    // needs-reboot: no GPU-client start; a user's explicit `--force` start (set session=on force=1) passes once
    if s.gpu && w.gate.effective() == GateLevel::NeedsReboot && !(id == "monado" && w.intent.overrides.contains_key("__force")) {
        push_note(p, "gate", true, format!("{} not restarted: the GPU needs a reboot", s.label));
        return;
    }
    if id == "wayvr" && class == Some(ExitClass::Crash) && w.wayvr_xid_at.is_some_and(|x| c.down_at.is_some_and(|d| (d - x).abs() <= 10.0)) {
        push_note(p, "wayvr-xid", false, "WayVR crashed with a GPU fault (Xid 31): not restarted automatically. Fix restarts it.".into());
        return;
    }
    if id == "monado" {
        if w.facts.watchdog {
            p.steps.push(Step::new(id, Act::Wait { until: None }, Level::L0, "the launcher's watchdog restarts it"));
            return;
        }
        if w.mode() == Mode::Vr {
            push_note(p, "vr-crash", true, "VR runtime stopped during a game: restart it from the app when ready".into());
            return;
        }
        if !w.switches.core {
            push_note(p, "core-off", false, "VR runtime is down (automatic core restarts are off)".into());
            return;
        }
    } else if matches!(s.class, Class::Leaf | Class::XrClient | Class::Core) && !w.switches.leaves && s.class != Class::Core {
        push_note(p, "leaves-off", false, format!("{} is down (automatic restarts are off)", s.label));
        return;
    }
    let spend = class == Some(ExitClass::Crash);
    let pol = w.budgets.policy(s.class);
    let down_at = c.down_at.unwrap_or(w.now);
    let verdict = if spend { c.restart.verdict(&pol, w.now, down_at) } else if c.restart.breaker_open() { Verdict::No("breaker open".into()) } else { Verdict::Now };
    let level = if s.class == Class::Core { Level::L3 } else { Level::L2 };
    match verdict {
        Verdict::Now => {
            let mut st = Step::new(id, Act::Start, level, match class {
                Some(ExitClass::Crash) => format!("crashed: restart ({} of {})", c.restart.used(&pol, w.now) + 1, pol.max),
                Some(ExitClass::Collateral) => "exited with its dependency: restart (no budget)".to_string(),
                Some(ExitClass::Expected) => "stopped by omarchyvrd: restore".to_string(),
                _ => "wanted: start".to_string(),
            });
            st.spend = spend;
            p.steps.push(st);
        }
        Verdict::At(t) => p.steps.push(Step::new(id, Act::Wait { until: Some(t) }, Level::L0, format!("backoff: restart in {:.0} s", t - w.now))),
        Verdict::No(why) => push_note(p, "breaker", true, format!("{} keeps failing ({why}): Fix in the app", s.label)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::Gate;
    use crate::model::{By, Desired, Intent};
    use crate::policy::Budgets;
    use crate::probe::{Obs, SessionFacts};
    use crate::spec::SPECS;
    use crate::state::{Switches, World};
    use std::collections::BTreeMap;

    fn obs(up: &[(&'static str, bool)], pids: u32) -> BTreeMap<Id, Obs> {
        let mut m: BTreeMap<Id, Obs> = SPECS.iter().map(|s| (s.id, Obs { up: Some(false), ..Default::default() })).collect();
        for &(id, v) in up {
            let pid = spec(id).and_then(|s| s.pid).map(|_| pids + id.len() as u32);
            m.insert(id, Obs { up: Some(v), pid: if v { pid } else { None }, start_ticks: Some(pids as u64), ready: Some(true), ..Default::default() });
        }
        m
    }

    const DESK: &[&str] = &["usb.hid", "usb.cam", "usb.eye", "keepalive", "display.dp1", "gpu.gate", "host.caps", "host.mhs", "host.runtime-link", "broker", "monado", "eye", "sixdof", "wayvr", "passthrough", "ctrltrack", "perf", "eye.laser", "preset"];

    fn set(down: &[&str]) -> Vec<(&'static str, bool)> {
        DESK.iter().map(|id| (*id, !down.contains(id))).collect()
    }

    fn desk_world() -> World {
        let mut w = World::new(Desired::default(), Budgets::default(), Switches { enabled: true, ..Default::default() }, Gate::new("b"), Intent::default());
        let f = SessionFacts { mode: Some(Mode::Desk), ..Default::default() };
        w.observe(0.0, &obs(&set(&[]), 100), &f, &|_| false);
        w
    }

    fn step(w: &mut World, t: f64, down: &[&str]) -> Plan {
        let f = w.facts.clone();
        w.observe(t, &obs(&set(down), 100), &f, &|_| false);
        reconcile(w)
    }

    #[test]
    fn a_healthy_session_needs_nothing() {
        let w = desk_world();
        assert_eq!(reconcile(&w), Plan::default());
        assert_eq!(w.intent.by, By::Adopted);
    }

    #[test]
    fn a_monado_crash_restarts_the_subtree_after_backoff() {
        let mut w = desk_world();
        let p = step(&mut w, 10.0, &["monado", "wayvr", "passthrough", "ctrltrack", "perf"]);
        assert_eq!(p.action(), None);
        assert!(matches!(p.next().unwrap().act, Act::Wait { until: Some(t) } if (t - 40.0).abs() < 1e-6), "{p:?}");
        let p = step(&mut w, 40.0, &["monado", "wayvr", "passthrough", "ctrltrack", "perf"]);
        let a = p.action().unwrap();
        assert_eq!((a.id, &a.act, a.spend), ("monado", &Act::Start, true));
        assert!(p.steps.iter().all(|s| s.id != "wayvr" || !s.act.acts()), "clients come with the launcher start");
    }

    #[test]
    fn a_game_crash_only_notifies() {
        let mut w = desk_world();
        w.facts.mode = Some(Mode::Vr);
        let p = step(&mut w, 10.0, &["monado", "wayvr", "passthrough", "ctrltrack", "perf"]);
        assert_eq!(p.action(), None);
        assert!(p.notices().any(|s| s.id == "monado"));
    }

    #[test]
    fn a_wayvr_crash_is_an_l2_restart_and_needs_reboot_blocks_it() {
        let mut w = desk_world();
        let pid = w.c("wayvr").pid.unwrap();
        w.exit_status.insert(pid, (None, Some(libc::SIGSEGV)));
        let p = step(&mut w, 10.0, &["wayvr", "perf"]);
        assert!(matches!(p.next().unwrap().act, Act::Wait { .. }), "5 s backoff: {p:?}");
        let p = step(&mut w, 15.0, &["wayvr", "perf"]);
        let a = p.action().unwrap();
        assert_eq!((a.id, a.level, a.spend), ("wayvr", Level::L2, true));
        w.raise_gate(GateLevel::NeedsReboot, "test");
        let p = reconcile(&w);
        assert_eq!(p.action(), None);
        assert!(p.notices().any(|s| s.why.contains("reboot")));
    }

    #[test]
    fn an_unplug_in_desk_stops_cleanly_and_replug_restores() {
        let mut w = desk_world();
        let all: Vec<&str> = DESK.iter().copied().filter(|id| !matches!(*id, "gpu.gate" | "host.caps" | "host.mhs" | "host.runtime-link" | "keepalive")).collect();
        let p = step(&mut w, 1.0, &["usb.hid", "usb.cam", "usb.eye", "display.dp1"]);
        assert!(p.action().is_none(), "debounce: {p:?}");
        let p = step(&mut w, 7.0, &["usb.hid", "usb.cam", "usb.eye", "display.dp1"]);
        assert_eq!(p.action().unwrap().act, Act::StopSubtree);
        // omarchyvrd stopped them: expected exits
        for id in ["monado", "wayvr", "passthrough", "ctrltrack", "perf", "broker"] {
            w.comps.get_mut(id).unwrap().stopping = true;
        }
        let p = step(&mut w, 9.0, &all);
        assert!(p.action().is_none());
        assert!(p.steps.iter().any(|s| s.why.contains("waiting for the headset")));
        assert_eq!(w.c("monado").last_exit.as_ref().unwrap().1, ExitClass::Expected);
        // replug: 4 s steady for the HID, then Monado restores (free)
        let rest: Vec<&str> = ["monado", "wayvr", "passthrough", "ctrltrack", "perf", "broker"].to_vec();
        step(&mut w, 300.0, &rest);
        let p = step(&mut w, 305.0, &rest);
        let a = p.action().unwrap();
        assert_eq!((a.id, &a.act, a.spend), ("monado", &Act::Start, false), "{p:?}");
    }

    #[test]
    fn an_unplug_in_a_game_never_stops_it() {
        let mut w = desk_world();
        w.facts.mode = Some(Mode::Vr);
        let p = step(&mut w, 7.0, &["usb.hid", "usb.cam", "usb.eye", "display.dp1"]);
        let p2 = step(&mut w, 14.0, &["usb.hid", "usb.cam", "usb.eye", "display.dp1"]);
        assert!(p.action().is_none() && p2.action().is_none(), "{p2:?}");
        assert!(p2.notices().any(|s| s.why.contains("game keeps running")));
    }

    #[test]
    fn a_camera_drop_waits_then_resets_once() {
        let mut w = desk_world();
        let d = ["usb.cam", "broker", "passthrough", "ctrltrack"];
        let p = step(&mut w, 1.0, &d);
        assert!(p.action().is_none());
        assert!(p.steps.iter().any(|s| s.why.contains("20 s")));
        let p = step(&mut w, 22.0, &d);
        let a = p.action().unwrap();
        assert_eq!((a.id, &a.act, a.spend), ("usb.cam", &Act::ResetHeadset, true));
        w.auto_reset_done = true;
        let p = reconcile(&w);
        assert!(p.action().is_none());
        assert!(p.notices().any(|s| s.why.contains("Restart headset once more")));
    }

    #[test]
    fn the_camera_back_by_itself_restarts_the_subtree() {
        let mut w = desk_world();
        step(&mut w, 1.0, &["usb.cam", "broker", "passthrough", "ctrltrack"]);
        let p = step(&mut w, 5.0, &["broker", "passthrough", "ctrltrack"]);
        let a = p.action().unwrap();
        assert_eq!((a.id, &a.act), ("broker", &Act::StopSubtree));
    }

    #[test]
    fn session_off_from_a_client_stops_leaves_first() {
        let mut w = desk_world();
        w.set_intent_off(By::User, true);
        let p = reconcile(&w);
        let a = p.action().unwrap();
        assert_eq!(a.act, Act::Stop);
        assert!(matches!(a.id, "perf" | "voice.act" | "voice.captions" | "voicehud" | "eye.laser"), "{}", a.id);
        assert_ne!(a.id, "monado");
    }

    #[test]
    fn resume_grace_and_watchdog_deferral() {
        let mut w = desk_world();
        w.resumed(1.0, 3600.0, 15.0);
        let p = step(&mut w, 2.0, &["monado"]);
        assert!(p.action().is_none());
        let mut w = desk_world();
        w.facts.watchdog = true;
        let p = step(&mut w, 100.0, &["monado", "wayvr", "passthrough", "ctrltrack", "perf"]);
        let f = SessionFacts { watchdog: true, mode: Some(Mode::Desk), ..Default::default() };
        w.observe(200.0, &obs(&set(&["monado", "wayvr", "passthrough", "ctrltrack", "perf"]), 100), &f, &|_| false);
        let p2 = reconcile(&w);
        assert!(p.action().is_none() && p2.action().is_none(), "{p2:?}");
    }
}
