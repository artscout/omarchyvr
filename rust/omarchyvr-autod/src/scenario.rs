//! `omarchyvrd scenario SCRIPT...`: replay a scripted timeline through the real [`Supervisor`] (the World, the pure
//! reconcile, budgets, the gate) with a simulated executor and simulated devices/processes. One script per
//! scenario of the plan's section 5 lives in `rust/omarchyvr-autod/sim/scenarios/`.
//!
//! ```text
//! config restart.enabled=true restart.core=false ...   omarchyvrd.toml keys as section.key=value
//! sim start_delay=8 reset_back=90 fail_start=0 monado_hangs=0 wayvr_lingers=0
//! start desk|vr|none [voice] [ai]       what runs at t=0 (adopted)
//! at T down ID...                       raw: gone (devices: unplugged; processes: died without a status)
//! at T up ID...                         raw: back (a process coming up by itself gets a new pid)
//! at T exit ID sig=N | code=N | user | unknown     a process exits (sig/code: as omarchyvrd's child)
//! at T klog TEXT...                     a kernel log line
//! at T monado-log TEXT...               a Monado log line
//! at T resume SECONDS                   the machine resumed after sleeping that long
//! at T degrade ID TEXT...               the probe reports ID degraded (empty TEXT: healthy again)
//! at T cmd session=on|off | force-start | toggle KEY on|off | fix ID | safe-mode on|off | ack-gpu
//! at T facts mode=desk|vr watchdog=0|1 user_stop=0|1
//! expect T ran ACT ID                   the step ACT (start, stop, stop-subtree, reset-headset, reapply) ran on ID
//!                                       (each match is used once)
//! expect T runs N                       N steps ran in total by T
//! expect T overall|gate|intent VALUE    overall summary / gate level / intent session (on|off|unset)
//! expect T health ID HEALTH             e.g. ok, blocked, down, failed, off, starting, degraded
//! expect T notified TEXT                some notice so far contains TEXT (the rest of the line)
//! expect T would ACT ID                 observe-only: the would-log names ACT on ID by T
//! end T
//! ```
use crate::dconfig::DaemonConfig;
use crate::supervisor::Supervisor;
use omarchyvr_components::gpu::Gate;
use omarchyvr_components::model::{By, Id, Intent, Mode};
use omarchyvr_components::probe::{Obs, SessionFacts};
use omarchyvr_components::reconcile::{Act, Step, SUBTREE};
use omarchyvr_components::spec::{spec, SPECS};
use omarchyvr_components::state::{Outcome, World};
use std::collections::{BTreeMap, BTreeSet};

/// Result of a run.
#[derive(Debug, Default)]
pub struct Outcome2 {
    pub log: Vec<String>,
    pub failures: Vec<String>,
}

#[derive(Debug, Clone)]
enum Ev {
    Down(Vec<String>),
    Up(Vec<String>),
    Exit(String, String),
    Klog(String),
    MonadoLog(String),
    Resume(f64),
    Cmd(Vec<String>),
    Facts(Vec<(String, String)>),
    Degrade(String, String),
}

const DESK: &[Id] = &["usb.hid", "usb.cam", "usb.eye", "keepalive", "display.dp1", "broker", "monado", "wayvr", "passthrough", "ctrltrack", "perf"];
const ALWAYS: &[Id] = &["gpu.gate", "host.caps", "host.mhs", "host.runtime-link"];

struct Sim {
    up: BTreeSet<&'static str>,
    pids: BTreeMap<&'static str, u32>,
    next_pid: u32,
    facts: SessionFacts,
    start_delay: f64,
    reset_back: Option<f64>,
    fail_start: u32,
    monado_hangs: bool,
    degraded: BTreeMap<String, String>,
}

impl Sim {
    fn bring_up(&mut self, id: &'static str) {
        if self.up.insert(id) {
            self.degraded.remove(id);
            self.next_pid += 1;
            self.pids.insert(id, self.next_pid);
        }
    }
    fn obs(&self) -> BTreeMap<Id, Obs> {
        let mut m = BTreeMap::new();
        for s in SPECS {
            let up = self.up.contains(s.id);
            let derived = match s.id {
                "eye" => self.up.contains("monado") && self.up.contains("usb.eye"),
                "sixdof" => self.up.contains("monado") && self.up.contains("broker"),
                "eye.laser" | "preset" => self.up.contains("wayvr"),
                _ => up,
            };
            let o = Obs {
                up: if s.id == "anchor.relocalize" { None } else { Some(derived || ALWAYS.contains(&s.id)) },
                pid: if up && s.pid.is_some() { self.pids.get(s.id).copied() } else { None },
                start_ticks: self.pids.get(s.id).map(|p| *p as u64),
                ready: if s.id == "monado" { Some(true) } else { None },
                detail: if up { "sim".into() } else { "sim: gone".into() },
                degraded: self.degraded.get(s.id).cloned(),
                ..Default::default()
            };
            m.insert(s.id, o);
        }
        m
    }
}

fn find(id: &str) -> Result<&'static str, String> {
    spec(id).map(|s| s.id).ok_or(format!("unknown component {id}"))
}

pub fn run(script: &str) -> Result<Outcome2, String> {
    let mut cfg = DaemonConfig::default();
    cfg.switches.enabled = true;
    let mut sim = Sim { up: BTreeSet::new(), pids: BTreeMap::new(), next_pid: 1000, facts: SessionFacts::default(), start_delay: 8.0, reset_back: Some(90.0), fail_start: 0, monado_hangs: false, degraded: BTreeMap::new() };
    let mut events: Vec<(f64, Ev)> = Vec::new();
    let mut expects: Vec<(f64, Vec<String>)> = Vec::new();
    let mut end: Option<f64> = None;
    let mut start: Option<(Mode, Vec<String>)> = None;
    for (n, raw) in script.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let err = |e: String| format!("line {}: {e}", n + 1);
        let w: Vec<String> = line.split_whitespace().map(str::to_string).collect();
        let num = |i: usize| w.get(i).and_then(|t| t.parse::<f64>().ok()).ok_or_else(|| err("want a time".into()));
        match w[0].as_str() {
            "config" => {
                for kv in &w[1..] {
                    let (k, v) = kv.split_once('=').ok_or_else(|| err(format!("bad {kv}")))?;
                    let (sect, key) = k.split_once('.').ok_or_else(|| err(format!("want section.key: {k}")))?;
                    cfg.set(sect, key, v).map_err(err)?;
                }
            }
            "sim" => {
                for kv in &w[1..] {
                    match kv.split_once('=') {
                        Some(("start_delay", v)) => sim.start_delay = v.parse().map_err(|_| err(kv.clone()))?,
                        Some(("reset_back", "never")) => sim.reset_back = None,
                        Some(("reset_back", v)) => sim.reset_back = Some(v.parse().map_err(|_| err(kv.clone()))?),
                        Some(("fail_start", v)) => sim.fail_start = v.parse().map_err(|_| err(kv.clone()))?,
                        Some(("monado_hangs", v)) => sim.monado_hangs = v == "1",
                        _ => return Err(err(format!("bad sim option {kv}"))),
                    }
                }
            }
            "start" => {
                let m = match w.get(1).map(String::as_str) {
                    Some("desk") => Mode::Desk,
                    Some("vr") => Mode::Vr,
                    Some("none") => {
                        start = None;
                        continue;
                    }
                    other => return Err(err(format!("start desk|vr|none, not {other:?}"))),
                };
                start = Some((m, w[2..].to_vec()));
            }
            "at" => {
                let t = num(1)?;
                let rest = &w[2..];
                let ids = |r: &[String]| -> Result<Vec<String>, String> {
                    for x in r {
                        find(x).map_err(err)?;
                    }
                    Ok(r.to_vec())
                };
                let ev = match rest.first().map(String::as_str) {
                    Some("down") => Ev::Down(ids(&rest[1..])?),
                    Some("up") => Ev::Up(ids(&rest[1..])?),
                    Some("exit") => {
                        let id = rest.get(1).ok_or_else(|| err("exit ID how".into()))?;
                        find(id).map_err(err)?;
                        Ev::Exit(id.clone(), rest.get(2).cloned().unwrap_or_else(|| "unknown".into()))
                    }
                    Some("klog") => Ev::Klog(rest[1..].join(" ")),
                    Some("monado-log") => Ev::MonadoLog(rest[1..].join(" ")),
                    Some("resume") => Ev::Resume(rest.get(1).and_then(|v| v.parse().ok()).ok_or_else(|| err("resume SECONDS".into()))?),
                    Some("cmd") => Ev::Cmd(rest[1..].to_vec()),
                    Some("degrade") => Ev::Degrade(find(rest.get(1).map(String::as_str).unwrap_or("")).map_err(err)?.to_string(), rest[2..].join(" ")),
                    Some("facts") => Ev::Facts(rest[1..].iter().filter_map(|kv| kv.split_once('=').map(|(a, b)| (a.to_string(), b.to_string()))).collect()),
                    other => return Err(err(format!("unknown event {other:?}"))),
                };
                events.push((t, ev));
            }
            "expect" => {
                let t = num(1)?;
                if w.len() < 4 && w.get(2).map(String::as_str) != Some("runs") {
                    return Err(err("expect T WHAT VALUE".into()));
                }
                expects.push((t, w[2..].to_vec()));
            }
            "end" => end = Some(num(1)?),
            other => return Err(err(format!("unknown line {other:?}"))),
        }
    }
    // the world
    let mut world = World::new(cfg.desired.clone(), cfg.budgets.clone(), cfg.switches.clone(), Gate::new("sim-boot"), Intent::default());
    if let Some((m, extra)) = &start {
        sim.facts.mode = Some(*m);
        let set: Vec<Id> = match m {
            Mode::Desk => DESK.to_vec(),
            Mode::Vr => DESK.iter().copied().filter(|id| !matches!(*id, "wayvr" | "passthrough" | "perf")).collect(),
        };
        for id in set {
            sim.bring_up(id);
        }
        for x in extra {
            match x.as_str() {
                "voice" => ["voice.listen", "voice.captions", "voice.act"].iter().for_each(|id| sim.bring_up(id)),
                "ai" => sim.bring_up("ai.server"),
                other => sim.bring_up(find(other)?),
            }
        }
    }
    if start.as_ref().is_some_and(|s| s.1.iter().any(|x| x == "ai")) {
        world.intent.overrides.insert("ai.server".into(), true);
    }
    let mut sup = Supervisor::new(world);
    let mut o = Outcome2::default();
    events.sort_by(|a, b| a.0.total_cmp(&b.0));
    let last = events.iter().map(|e| e.0).chain(expects.iter().map(|e| e.0)).fold(0.0, f64::max);
    let end = end.unwrap_or(last + 30.0);
    let mut ran: Vec<(f64, String, String, bool)> = Vec::new();
    let mut woulds: Vec<(f64, String)> = Vec::new();
    let mut notices: Vec<String> = Vec::new();
    // the running simulated step: (finishes at, step)
    let mut pending: Option<(f64, Step)> = None;
    let mut reset_until: Option<f64> = None;
    let mut ei = 0;
    let mut t = 0.0;
    while t <= end + 1e-9 {
        while ei < events.len() && events[ei].0 <= t + 1e-9 {
            let ev = events[ei].1.clone();
            ei += 1;
            match ev {
                Ev::Down(ids) => {
                    for id in ids {
                        sim.up.remove(find(&id)?);
                    }
                }
                Ev::Up(ids) => {
                    for id in ids {
                        sim.bring_up(find(&id)?);
                    }
                }
                Ev::Exit(id, how) => {
                    let id = find(&id)?;
                    let pid = sim.pids.get(id).copied().unwrap_or(0);
                    let mut kv = how.split('=');
                    match (kv.next(), kv.next()) {
                        (Some("sig"), Some(v)) => {
                            sup.w.exit_status.insert(pid, (None, v.parse().ok()));
                        }
                        (Some("code"), Some(v)) => {
                            sup.w.exit_status.insert(pid, (v.parse().ok(), None));
                        }
                        (Some("user"), _) => {
                            sup.w.exit_status.insert(pid, (Some(0), None));
                        }
                        _ => {}
                    }
                    sim.up.remove(id);
                }
                Ev::Klog(l) => {
                    sup.w.kernel_line(t, &l);
                }
                Ev::MonadoLog(l) => {
                    sup.w.monado_line(&l);
                }
                Ev::Resume(s) => sup.w.resumed(t, s, 15.0),
                Ev::Degrade(id, text) => {
                    if text.is_empty() {
                        sim.degraded.remove(&id);
                    } else {
                        sim.degraded.insert(id, text);
                    }
                }
                Ev::Facts(kvs) => {
                    for (k, v) in kvs {
                        match k.as_str() {
                            "mode" => sim.facts.mode = Mode::parse(&v),
                            "watchdog" => sim.facts.watchdog = v == "1",
                            "user_stop" => sim.facts.user_stop = v == "1",
                            _ => return Err(format!("unknown fact {k}")),
                        }
                    }
                }
                Ev::Cmd(c) => {
                    o.log.push(format!("{t:7.1}s  cmd {}", c.join(" ")));
                    match c.first().map(String::as_str) {
                        Some("session=on") => sup.set_session(true, By::User, None),
                        Some("session=off") => sup.set_session(false, By::User, None),
                        Some("force-start") => {
                            sup.set_session(true, By::User, None);
                            sup.w.intent.overrides.insert("__force".into(), true);
                        }
                        Some("toggle") => {
                            let on = c.get(2).map(String::as_str) == Some("on");
                            if let Err(e) = sup.toggle(c.get(1).map(String::as_str).unwrap_or(""), on) {
                                o.log.push(format!("{t:7.1}s  toggle refused: {e}"));
                            }
                        }
                        Some("fix") => sup.fix(c.get(1).map(String::as_str).unwrap_or(""))?,
                        Some("safe-mode") => sup.w.safe_mode = c.get(1).map(String::as_str) == Some("on"),
                        Some("ack-gpu") => {
                            sup.w.gate.ack();
                        }
                        other => return Err(format!("unknown cmd {other:?}")),
                    }
                }
            }
        }
        // the simulated executor
        if let Some(rt) = reset_until {
            if t + 1e-9 >= rt {
                reset_until = None;
                if sim.reset_back.is_some_and(f64::is_finite) {
                    sim.bring_up("usb.cam");
                }
            }
        }
        if let Some((at, st)) = pending.clone() {
            if t + 1e-9 >= at && reset_until.is_none() {
                pending = None;
                let outcome = sim_finish(&mut sim, &st);
                o.log.push(format!("{t:7.1}s  done {} -> {outcome:?}", st.line()));
                for l in sup.finished(&outcome, t) {
                    o.log.push(format!("{t:7.1}s  {l}"));
                }
            }
        }
        let obs = sim.obs();
        let facts = sim.facts.clone();
        sup.w.observe(t, &obs, &facts, &|_| false);
        let d = sup.decide();
        for e in &d.events {
            if !e.contains("\"event\":\"health\"") {
                o.log.push(format!("{t:7.1}s  {e}"));
            }
            if e.contains("\"event\":\"would\"") {
                woulds.push((t, e.clone()));
            }
            if e.contains("\"event\":\"notice\"") || e.contains("\"event\":\"critical\"") || e.contains("\"event\":\"would\"") {
                notices.push(e.clone());
            }
        }
        if let Some(st) = d.run {
            ran.push((t, st.act.name().to_string(), st.id.to_string(), false));
            let dur = sim_begin(&mut sim, &st, &mut reset_until, t);
            pending = Some((t + dur, st));
        }
        for (et, want) in &expects {
            if (et - t).abs() < 0.5 {
                if let Some(f) = check(&sup, want, &mut ran, &woulds, &notices, *et) {
                    o.failures.push(format!("at {et}s expected {}: {f}", want.join(" ")));
                }
            }
        }
        t += 1.0;
    }
    let h = sup.w.health();
    o.log.push(format!("{t:7.1}s  end: overall {} (steps run: {})", sup.w.overall(&h).name(), ran.len()));
    Ok(o)
}

/// Start a simulated step; returns how long it takes.
fn sim_begin(sim: &mut Sim, st: &Step, reset_until: &mut Option<f64>, t: f64) -> f64 {
    match st.act {
        Act::Start if st.id == "monado" => sim.start_delay,
        Act::Start => 2.0,
        Act::Stop => 1.0,
        Act::StopSubtree => 3.0,
        Act::ResetHeadset => {
            for id in SUBTREE.iter().chain(["monado"].iter()) {
                sim.up.remove(id);
            }
            sim.up.remove("usb.cam");
            *reset_until = sim.reset_back.map(|b| t + b);
            if reset_until.is_none() {
                *reset_until = Some(t + 150.0);
                sim.reset_back = Some(f64::INFINITY);
            }
            5.0
        }
        _ => 0.0,
    }
}

fn sim_finish(sim: &mut Sim, st: &Step) -> Outcome {
    match st.act {
        Act::Start => {
            if sim.fail_start > 0 {
                sim.fail_start -= 1;
                return Outcome::Failed("simulated start failure".into());
            }
            if st.id == "monado" {
                sim.bring_up("monado");
                // the launcher start brings the session's clients back
                for id in SUBTREE {
                    if sim.pids.contains_key(id) {
                        sim.bring_up(id);
                    }
                }
            } else {
                sim.bring_up(spec(st.id).unwrap().id);
            }
            Outcome::Ok
        }
        Act::Stop => {
            sim.up.remove(st.id);
            Outcome::Ok
        }
        Act::StopSubtree => {
            for id in SUBTREE {
                sim.up.remove(id);
            }
            if sim.monado_hangs {
                return Outcome::Hung("monado-service did not exit within 30 s".into());
            }
            sim.up.remove("monado");
            Outcome::Ok
        }
        Act::ResetHeadset => {
            if sim.reset_back == Some(f64::INFINITY) {
                sim.reset_back = None;
                Outcome::Failed("camera still missing after 150 s".into())
            } else {
                Outcome::Ok
            }
        }
        _ => Outcome::Ok,
    }
}

fn check(sup: &Supervisor, want: &[String], ran: &mut [(f64, String, String, bool)], woulds: &[(f64, String)], notices: &[String], t: f64) -> Option<String> {
    let what = want[0].as_str();
    let arg = |i: usize| want.get(i).map(String::as_str).unwrap_or("");
    let h = sup.w.health();
    match what {
        "ran" => {
            let hit = ran.iter_mut().find(|r| !r.3 && r.0 <= t + 1e-9 && r.1 == arg(1) && r.2 == arg(2));
            match hit {
                Some(r) => {
                    r.3 = true;
                    None
                }
                None => Some(format!("ran so far: {:?}", ran.iter().map(|r| format!("{:.0}s {} {}", r.0, r.1, r.2)).collect::<Vec<_>>())),
            }
        }
        "runs" => {
            let n = ran.iter().filter(|r| r.0 <= t + 1e-9).count();
            (n.to_string() != arg(1)).then(|| format!("{n} ran: {:?}", ran.iter().map(|r| format!("{:.0}s {} {}", r.0, r.1, r.2)).collect::<Vec<_>>()))
        }
        "overall" => {
            let o = sup.w.overall(&h).name();
            (o != arg(1)).then(|| format!("got {o}; {:?}", h.iter().filter(|(_, v)| v.name() != "ok" && v.name() != "off").collect::<Vec<_>>()))
        }
        "gate" => {
            let g = sup.w.gate.effective().name();
            (g != arg(1)).then(|| format!("got {g}"))
        }
        "intent" => {
            let i = sup.w.intent.session_name();
            (i != arg(1)).then(|| format!("got {i}"))
        }
        "health" => {
            let got = h.get(arg(1)).map(|x| x.name()).unwrap_or("?");
            (got != arg(2)).then(|| format!("got {got} ({:?})", h.get(arg(1))))
        }
        "notified" => {
            let text = want[1..].join(" ");
            (!notices.iter().any(|n| n.contains(&text))).then(|| format!("notices: {notices:?}"))
        }
        "would" => {
            let needle = format!("{} {}", arg(1), arg(2));
            (!woulds.iter().any(|(wt, l)| *wt <= t + 1e-9 && l.contains(&needle))).then(|| format!("woulds: {woulds:?}"))
        }
        "safe-auto" => {
            let got = if sup.w.safe_auto.is_some() { "yes" } else { "no" };
            (got != arg(1)).then(|| format!("got {got}"))
        }
        other => Some(format!("unknown expectation {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_script(name: &str, text: &str) {
        let o = run(text).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(o.failures.is_empty(), "{name}: {:#?}\n{}", o.failures, o.log.join("\n"));
    }

    #[test]
    fn shipped_scenarios_pass() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sim/scenarios");
        let mut n = 0;
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("scn") {
                check_script(&p.display().to_string(), &std::fs::read_to_string(&p).unwrap());
                n += 1;
            }
        }
        assert!(n >= 12, "only {n} scenarios");
    }

    #[test]
    fn bad_scripts_are_errors() {
        assert!(run("at x down usb.hid").is_err());
        assert!(run("at 1 down bogus").is_err());
        assert!(run("config restart.nope=1").is_err());
        assert!(run("frobnicate").is_err());
    }

    #[test]
    fn a_wrong_expectation_is_reported() {
        let o = run("start desk\nexpect 5 overall down\n").unwrap();
        assert_eq!(o.failures.len(), 1, "{:?}", o.log);
    }
}
