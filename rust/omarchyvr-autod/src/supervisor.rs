//! omarchyvrd's reconciler driver: the [`World`] plus what the daemon keeps around it (event lines, the "would" log,
//! notices to send, captured argv for respawns) and the files it mirrors (`state.json`, `intent`, `gpu-gate`,
//! `safe-mode-auto`). [`Supervisor::decide`] is deterministic: the daemon and the scenario simulator call the same
//! code and differ only in how a chosen step is executed.
use omarchyvr_components::gpu::Gate;
use omarchyvr_components::json::q;
use omarchyvr_components::model::{By, GateLevel, Id};
use omarchyvr_components::reconcile::{reconcile, Plan, Step};
use omarchyvr_components::state::{Notice, Outcome, World};
use omarchyvr_components::util::{write_atomic, Roots};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// How a process was started (captured from /proc while it ran), so a crashed leaf restarts the same way.
#[derive(Debug, Clone, PartialEq)]
pub struct Captured {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: Option<PathBuf>,
}

/// What one decision round produced.
#[derive(Debug, Default)]
pub struct Decision {
    /// The step to execute now (never in observe-only mode).
    pub run: Option<Step>,
    /// Notices to deliver (desktop / headset); empty in observe-only mode.
    pub notify: Vec<Notice>,
    /// Event lines for subscribers and the log.
    pub events: Vec<String>,
}

pub struct Supervisor {
    pub w: World,
    pub plan: Plan,
    last_health: BTreeMap<Id, String>,
    last_would: Option<String>,
    pub captured: BTreeMap<Id, Captured>,
    /// A step is running in the executor.
    pub running: Option<Step>,
    /// Components the user acted on (toggle, Fix) and when: their steps run even in observe-only / safe mode
    /// ("explicit user commands still work"), for 60 s.
    pub explicit: BTreeMap<Id, f64>,
}

pub fn event_line(t: f64, kind: &str, id: &str, text: &str) -> String {
    format!("{{\"event\":{},\"t\":{:.1},\"id\":{},\"text\":{}}}", q(kind), t, q(id), q(text))
}

impl Supervisor {
    pub fn new(w: World) -> Supervisor {
        Supervisor { w, plan: Plan::default(), last_health: BTreeMap::new(), last_would: None, captured: BTreeMap::new(), running: None, explicit: BTreeMap::new() }
    }

    /// Reconcile and choose. Call after every observation, event or executor result.
    pub fn decide(&mut self) -> Decision {
        let now = self.w.now;
        let mut d = Decision::default();
        let observe_only = self.w.observe_only();
        // health changes
        let h = self.w.health();
        for (id, hh) in &h {
            let name = match hh.why() {
                Some(w) => format!("{} ({w})", hh.name()),
                None => hh.name().to_string(),
            };
            if self.last_health.get(id) != Some(&name) {
                if self.last_health.contains_key(id) || hh.name() != "off" {
                    d.events.push(format!(
                        "{{\"event\":\"health\",\"t\":{now:.1},\"id\":{},\"health\":{},\"why\":{}}}",
                        q(id),
                        q(hh.name()),
                        hh.why().map_or("null".into(), q)
                    ));
                }
                self.last_health.insert(id, name);
            }
        }
        // the world's own notices (exits, gate, adoption)
        for n in std::mem::take(&mut self.w.notices) {
            d.events.push(event_line(now, if n.critical { "critical" } else if n.notify { "notice" } else { "log" }, &n.id, &n.text));
            if n.notify && !observe_only {
                d.notify.push(n);
            }
        }
        self.plan = reconcile(&self.w);
        // the plan's notices: told once per key (also in observe-only mode, so the would-log does not repeat)
        let notes: Vec<Step> = self.plan.notices().cloned().collect();
        for st in notes {
            self.w.begin(&st, now);
            let critical = matches!(st.act, omarchyvr_components::reconcile::Act::Notify { critical: true, .. });
            if observe_only {
                d.events.push(event_line(now, "would", st.id, &format!("notify: {}", st.why)));
            } else {
                d.events.push(event_line(now, if critical { "critical" } else { "notice" }, st.id, &st.why));
                d.notify.push(Notice { id: st.id.into(), text: st.why.clone(), notify: true, critical });
            }
        }
        let action = self.plan.action().cloned();
        let line = action.as_ref().map(Step::line);
        if self.running.is_none() && now >= self.w.settle_until {
            if let Some(st) = action {
                let user_asked = self.explicit.get(st.id).is_some_and(|t| now - t < 60.0);
                if observe_only && !user_asked {
                    if self.last_would != line {
                        d.events.push(event_line(now, "would", st.id, &st.line()));
                    }
                } else if self.w.begin(&st, now) {
                    d.events.push(event_line(now, "step", st.id, &st.line()));
                    self.explicit.remove(st.id);
                    self.running = Some(st.clone());
                    d.run = Some(st);
                } else {
                    for n in std::mem::take(&mut self.w.notices) {
                        d.events.push(event_line(now, "critical", &n.id, &n.text));
                        d.notify.push(n);
                    }
                }
            }
        }
        self.last_would = line;
        d
    }

    /// The executor finished the running step.
    pub fn finished(&mut self, outcome: &Outcome, now: f64) -> Vec<String> {
        let mut ev = Vec::new();
        if let Some(st) = self.running.take() {
            self.w.now = now;
            self.w.finish(&st, outcome, now);
            ev.push(event_line(now, "done", st.id, &format!("{}: {outcome:?}", st.line())));
        }
        ev
    }

    /// A client set the intent.
    pub fn set_session(&mut self, on: bool, by: By, mode: Option<omarchyvr_components::model::Mode>) {
        if on {
            self.w.intent.session = Some(true);
            self.w.intent.by = by;
            if let Some(m) = mode {
                self.w.intent.mode = m;
            }
            self.w.intent.at = omarchyvr_components::util::unix_now();
            self.w.intent.overrides.remove("__stop");
            // a session a client asked for: omarchyvrd starts what the desired state says
            self.w.intent.overrides.insert("__owned".into(), true);
        } else {
            self.w.set_intent_off(by, true);
        }
    }

    /// A CLI start (`set session=on by=cli` without toggles): the session's components are what the launcher's
    /// recorded argv asks for, as per-session overrides (omarchyvrd.toml stays the default without an argv).
    pub fn apply_start_argv(&mut self, argv: Option<(String, Vec<String>)>) -> bool {
        let Some((kind, args)) = argv else { return false };
        let d = desired_from_argv(&kind, &args, &self.w.desired);
        self.w.intent.mode = d.mode;
        for s in omarchyvr_components::spec::SPECS.iter().filter(|s| s.scope == omarchyvr_components::spec::Scope::Session && !matches!(s.kind, omarchyvr_components::model::Kind::Device | omarchyvr_components::model::Kind::Job)) {
            self.w.intent.overrides.insert(s.id.to_string(), (s.rule)(&d));
        }
        true
    }

    /// A client toggled a component for this session. Returns the ids it applies to, or an error.
    pub fn toggle(&mut self, key: &str, on: bool) -> Result<Vec<Id>, String> {
        let ids = omarchyvr_components::spec::toggle_ids(key);
        let ids: Vec<Id> = if ids.is_empty() { omarchyvr_components::spec::spec(key).map(|s| vec![s.id]).ok_or(format!("unknown component {key}"))? } else { ids.to_vec() };
        for id in &ids {
            if let Some(omarchyvr_components::model::Toggle::Restart(p)) = omarchyvr_components::spec::spec(id).map(|s| s.toggle) {
                if *id != "monado" && self.w.c(id).up != Some(on) {
                    return Err(format!("{id} needs a restart of {p}: it applies at the next start (never automatically)"));
                }
            }
            self.w.intent.overrides.insert(id.to_string(), on);
            self.explicit.insert(id, self.w.now);
            // a user toggle is new evidence
            if let Some(c) = self.w.comps.get_mut(id) {
                c.restart.reset();
                c.told.clear();
                if on {
                    c.last_exit = None;
                    c.seen_up = true;
                }
            }
        }
        Ok(ids)
    }

    /// `fix <id>`: reset the breaker and the notices, so the next ladder step runs once.
    pub fn fix(&mut self, id: &str) -> Result<(), String> {
        let s = omarchyvr_components::spec::spec(id).ok_or(format!("unknown component {id}"))?;
        let c = self.w.comps.get_mut(s.id).ok_or("unknown")?;
        c.restart.reset();
        c.told.clear();
        if matches!(c.last_exit.as_ref().map(|e| e.1), Some(omarchyvr_components::policy::ExitClass::Unknown) | Some(omarchyvr_components::policy::ExitClass::User)) {
            c.last_exit = Some((self.w.now, omarchyvr_components::policy::ExitClass::Collateral, "Fix".into()));
        }
        c.seen_up = true;
        self.w.intent.overrides.remove(s.id);
        self.explicit.insert(s.id, self.w.now);
        if s.id == "usb.cam" {
            self.w.auto_reset_done = false;
        }
        if s.id == "wayvr" {
            self.w.wayvr_xid_at = None;
        }
        if self.w.safe_auto.is_some() && id == "omarchyvrd" {
            self.w.safe_auto = None;
        }
        Ok(())
    }

    /// The snapshot (schema v1, docs/PIMAXD.md). `extra` is appended inside the object (`,"autod":{...}`).
    pub fn snapshot(&self, extra: &str) -> String {
        omarchyvr_components::snapshot::json(&self.w, &self.plan, extra)
    }
}

/// ai.toml's `voice_local_models` (None: not set or no file).
pub fn local_models_switch(ai_toml: &Path) -> Option<bool> {
    let text = std::fs::read_to_string(ai_toml).ok()?;
    let c = omarchyvr_config::ai::AiConfig::parse(&text).ok()?;
    // the app's choices (Local models On / Off, Lara in front of an online brain): the AI servers have work in Work
    (c.voice_local_models.is_some() || c.voice_lara.is_some()).then(|| c.ai_servers_wanted())
}

/// What a launcher start (`<session dir>/start-argv.txt`: kind, args) asks for. The argv is authoritative for the
/// launcher's own flags (an absent opt-in flag means off); what the launcher has no flag for (ai_server,
/// floor_follow, eye_laser) comes from `base` (omarchyvrd.toml).
pub fn desired_from_argv(kind: &str, args: &[String], base: &omarchyvr_components::model::Desired) -> omarchyvr_components::model::Desired {
    use omarchyvr_components::model::{Eye, Mode};
    let has = |f: &str| args.iter().any(|a| a == f);
    let val = |f: &str| {
        args.iter().enumerate().find_map(|(i, a)| {
            if a == f {
                args.get(i + 1).cloned()
            } else {
                a.strip_prefix(&format!("{f}=")).map(str::to_string)
            }
        })
    };
    let mut d = base.clone();
    let desk = kind != "vr";
    d.mode = if desk { Mode::Desk } else { Mode::Vr };
    d.sixdof = has("--6dof");
    d.ctrltrack = d.sixdof && has("--ctrl-track");
    d.hands = d.sixdof && has("--hands");
    // light game mode (`vr start --light`): the launcher itself drops the wrist readout, eye tracking and the room anchor
    let light = !desk && has("--light");
    d.anchor = has("--anchor") && !light;
    d.eye = if has("--no-eye") || light {
        Eye::Off
    } else if has("--eye") {
        Eye::Pointer
    } else if desk {
        Eye::Silent
    } else {
        Eye::Off
    };
    if desk {
        d.passthrough = !has("--no-passthrough");
        d.perf = !has("--no-perf");
        d.voice = has("--voice");
        d.voicehud = d.voice && has("--voice-hud");
        d.preset = val("--preset").unwrap_or_else(|| "none".into());
        d.screens = val("--screens").and_then(|v| v.parse::<u8>().ok()).filter(|n| (1..=4).contains(n)).unwrap_or(4);
    } else {
        d.passthrough = false;
        d.perf = has("--perf") && !light;
        d.voice = false;
        d.voicehud = false;
        d.preset = "none".into();
        d.screens = 1;
    }
    d
}

/// The files omarchyvrd mirrors under `$XDG_RUNTIME_DIR/omarchyvrd` and `~/.config/omarchyvr`.
pub struct Files {
    pub dir: PathBuf,
    pub safe_flag: PathBuf,
    /// ai.toml: "Local models: On / Off" decides whether ai.server is wanted in Work (the app's Local AI page).
    pub ai_toml: PathBuf,
    last_intent: String,
    last_gate: String,
}

impl Files {
    pub fn new(r: &Roots) -> Files {
        Files { dir: r.omarchyvrd_dir(), safe_flag: r.config.join("safe-mode"), ai_toml: r.config.join("ai.toml"), last_intent: String::new(), last_gate: String::new() }
    }
    pub fn gate_path(&self) -> PathBuf {
        self.dir.join("gpu-gate")
    }

    /// Read what the files say into the world: the safe-mode flag, the gate latch (the launcher raises it too),
    /// the intent (at start-up).
    pub fn load(&mut self, w: &mut World, startup: bool) {
        w.safe_mode = self.safe_flag.exists();
        // the app's "Local models" switch (when set) wins over omarchyvrd.toml's ai_server
        if let Some(on) = local_models_switch(&self.ai_toml) {
            w.desired.ai_server = on;
        }
        let disk = Gate::load(&self.gate_path(), &w.gate.boot_id);
        // the launcher found GPU-fault evidence at the same level as our kill-only latch: strict again
        let upgrade = disk.fault && disk.level == GateLevel::NeedsReboot && w.gate.kill_only();
        if disk.level > w.gate.level || upgrade {
            let why = disk.evidence.last().cloned().unwrap_or_else(|| "latched by the launcher".into());
            if disk.kill_only() {
                w.raise_gate_kill(disk.level, &why);
            } else {
                w.raise_gate(disk.level, &why);
            }
        }
        if startup {
            if disk.acked && disk.level == GateLevel::Caution && w.gate.level == GateLevel::Caution {
                w.gate.acked = true;
            }
            if let Some(i) = omarchyvr_components::intent::read(&self.dir.join("intent")) {
                w.intent = i;
            }
            if let Some(r) = omarchyvr_components::util::read_trim(&self.dir.join("safe-mode-auto")).filter(|s| !s.is_empty()) {
                w.safe_auto = Some(r);
            }
        }
    }

    /// Write the snapshot (every round: `omarchyvr-health` treats it as fresh for 10 s), and the intent / gate / auto
    /// safe-mode files when they changed.
    pub fn save(&mut self, w: &World, snapshot: &str) {
        let _ = write_atomic(&self.dir.join("state.json"), format!("{snapshot}\n").as_bytes());
        let it = omarchyvr_components::intent::to_text(&w.intent);
        if it != self.last_intent {
            let _ = omarchyvr_components::intent::write(&self.dir.join("intent"), &w.intent);
            self.last_intent = it;
        }
        let gt = w.gate.to_text();
        if gt != self.last_gate {
            if w.gate.level != GateLevel::Ok || w.gate.acked {
                let _ = w.gate.save(&self.gate_path());
            }
            self.last_gate = gt;
        }
        let auto = self.dir.join("safe-mode-auto");
        match &w.safe_auto {
            Some(r) => {
                if !auto.exists() {
                    let _ = write_atomic(&auto, r.as_bytes());
                }
            }
            None => {
                let _ = std::fs::remove_file(&auto);
            }
        }
    }

    /// Record this start; true when omarchyvrd restarted more than 3 times in 5 min (a restart storm: safe mode).
    pub fn record_start(&self, unix_now: f64) -> bool {
        let p = self.dir.join("starts");
        let mut v: Vec<f64> = std::fs::read_to_string(&p).unwrap_or_default().lines().filter_map(|l| l.trim().parse().ok()).filter(|t| unix_now - t < 300.0).collect();
        v.push(unix_now);
        let text: String = v.iter().map(|t| format!("{t:.0}\n")).collect();
        let _ = write_atomic(&p, text.as_bytes());
        v.len() > 3
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omarchyvr_components::model::{By, Desired, Eye, Mode};

    fn sv(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_launcher_argv_decides_the_session_components() {
        let base = Desired { voice: true, ai_server: true, ..Desired::default() };
        let d = desired_from_argv("desk", &sv(&["--6dof", "--ctrl-track", "--voice", "--preset", "work-3", "--no-eye", "--no-perf"]), &base);
        assert_eq!((d.mode, d.sixdof, d.ctrltrack, d.voice, d.voicehud, d.perf, d.passthrough), (Mode::Desk, true, true, true, false, false, true));
        assert_eq!((d.eye, d.preset.as_str(), d.anchor, d.ai_server), (Eye::Off, "work-3", false, true));
        // opt-in flags absent mean off, whatever omarchyvrd.toml says
        let d = desired_from_argv("desk", &sv(&["--no-notify"]), &base);
        assert!(!d.sixdof && !d.ctrltrack && !d.voice && d.perf && d.eye == Eye::Silent && d.preset == "none");
        let d = desired_from_argv("vr", &sv(&["--6dof", "--perf", "--anchor", "--eye"]), &base);
        assert_eq!((d.mode, d.sixdof, d.perf, d.anchor, d.passthrough, d.voice, d.eye), (Mode::Vr, true, true, true, false, false, Eye::Pointer));
        assert_eq!(desired_from_argv("desk", &sv(&["--preset=cinema"]), &base).preset, "cinema");
        assert_eq!(desired_from_argv("desk", &sv(&["--preset", "cinema-huge"]), &base).preset, "cinema-huge");
        // --hands is an opt-in leaf of 6DoF: kept from the argv, off without it (whatever omarchyvrd.toml says)
        let base_h = Desired { hands: true, ..Desired::default() };
        assert!(desired_from_argv("desk", &sv(&["--6dof", "--hands", "--hands-roles"]), &base).hands);
        assert!(!desired_from_argv("desk", &sv(&["--6dof"]), &base_h).hands);
        assert!(!desired_from_argv("vr", &sv(&["--hands"]), &base).hands, "needs --6dof");
        let l = desired_from_argv("vr", &sv(&["--6dof", "--perf", "--anchor", "--eye", "--light"]), &base);
        assert!(l.sixdof && !l.perf && !l.anchor && l.eye == Eye::Off, "light game mode keeps only what a game needs");
    }

    #[test]
    fn a_cli_start_takes_its_components_from_start_argv() {
        let t = omarchyvr_components::util::TempDir::new("cli-argv");
        let session = t.path().join("session");
        omarchyvr_components::util::put(&session, "start-argv.txt", "desk\n--6dof\n--voice\n--no-perf\n");
        let base = Desired { perf: true, voice: false, sixdof: false, ..Desired::default() };
        let w = World::new(base, Default::default(), Default::default(), Gate::new("b"), Default::default());
        let mut sup = Supervisor::new(w);
        let req = crate::ipc::parse_request(r#"{"cmd":"set","session":"on","mode":"desk","by":"cli"}"#).unwrap();
        let crate::ipc::Request::Supervise(c) = req else { panic!() };
        let flag = t.path().join("safe-mode");
        let reply = crate::ipc::apply_supervise(&mut sup, &c, &flag, std::path::Path::new("/nonexistent/pimax"), &session);
        assert!(reply.contains("\"ok\":true"), "{reply}");
        assert_eq!(sup.w.intent.by, By::Cli);
        assert!(sup.w.owned_session());
        for id in ["monado", "broker", "sixdof", "voice.listen", "voice.captions", "voice.act", "wayvr", "passthrough"] {
            assert!(sup.w.wanted(id), "{id}");
        }
        for id in ["perf", "ctrltrack", "voicehud", "anchor.relocalize"] {
            assert!(!sup.w.wanted(id), "{id}");
        }
        // without an argv, omarchyvrd.toml stays the default
        let w = World::new(Desired { perf: true, ..Desired::default() }, Default::default(), Default::default(), Gate::new("b"), Default::default());
        let mut sup = Supervisor::new(w);
        let empty = t.path().join("empty");
        crate::ipc::apply_supervise(&mut sup, &c, &flag, std::path::Path::new("/nonexistent/pimax"), &empty);
        assert!(sup.w.wanted("perf") && !sup.w.wanted("voice.listen"));
        // a set with toggles keeps them and ignores the argv
        let w = World::new(Desired::default(), Default::default(), Default::default(), Gate::new("b"), Default::default());
        let mut sup = Supervisor::new(w);
        let req = crate::ipc::parse_request(r#"{"cmd":"set","session":"on","by":"cli","perf":false}"#).unwrap();
        let crate::ipc::Request::Supervise(c) = req else { panic!() };
        crate::ipc::apply_supervise(&mut sup, &c, &flag, std::path::Path::new("/nonexistent/pimax"), &session);
        assert!(!sup.w.wanted("voice.listen") && !sup.w.wanted("perf"));
    }
}
