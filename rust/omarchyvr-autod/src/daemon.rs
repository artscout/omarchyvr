//! The daemon loop (omarchyvrd, alias omarchyvr-autod). One thread waits on a channel (no busy loop):
//! * auto mode as before: every `poll_s` the signals are collected, the machine steps, its actions run in workers
//!   (dry run by default); its starts/stops also set the intent (`by=auto`);
//! * the reconciler: a probe thread (every 2 s, and at once on a hint) sends observations; uevents, pidfd exits,
//!   the kernel log follower, reaped children and resumes are hints/inputs; [`Supervisor::decide`] picks at most
//!   one step, which an executor worker runs (never in observe-only mode, the default);
//! * the socket: one thread per connection.
//!
//! Exiting never stops a session; children omarchyvrd respawned are detached (setsid) and survive it too.
use crate::config::{Config, Mode};
use crate::dconfig::DaemonConfig;
use crate::executor::{self, Ctx, Inhibit, Reaped};
use crate::ipc::{self, Request};
use crate::machine::{Action, Input, Kind, Machine};
use crate::sources::{Collector, Paths};
use crate::supervisor::{Captured, Files, Supervisor};
use omarchyvr_components::events::{Ev, KlogFollower, PidWatch, Resume, UeventSocket};
use omarchyvr_components::model::{By, Id};
use omarchyvr_components::probe::{Obs, SessionFacts};
use omarchyvr_components::state::{Outcome, World};
use omarchyvr_components::util::Roots;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

enum Msg {
    Req(Request, Sender<String>),
    Subscribe(Sender<String>),
    Done(bool, String),
    /// An event line produced by a worker thread (pairing progress).
    Event(String),
    Probe(Box<ProbeRound>),
    Src(Ev),
    StepDone(Outcome),
    Reaped(Reaped),
}

/// One probe round.
pub struct ProbeRound {
    obs: BTreeMap<Id, Obs>,
    facts: SessionFacts,
    captures: Vec<(Id, Captured)>,
    monado_lines: Vec<String>,
    log_crash: Vec<Id>,
}

pub fn log(m: &str) {
    let _ = writeln!(std::io::stderr(), "{} omarchyvrd: {m}", pimax::sys::local_hms());
}

fn serve_conn(s: UnixStream, tx: Sender<Msg>) {
    let Ok(mut w) = s.try_clone() else { return };
    for line in BufReader::new(s).lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        match ipc::parse_request(&line) {
            Err(e) => {
                if writeln!(w, "{}", ipc::error_line(&e)).is_err() {
                    return;
                }
            }
            Ok(Request::Events) => {
                let (etx, erx) = channel();
                if tx.send(Msg::Subscribe(etx)).is_err() {
                    return;
                }
                for ev in erx {
                    if writeln!(w, "{ev}").is_err() {
                        return;
                    }
                }
                return;
            }
            Ok(Request::Wait { what, timeout_s }) => {
                // poll the state until the condition holds or the time is up
                let t0 = Instant::now();
                let reply = loop {
                    let (rtx, rrx) = channel();
                    if tx.send(Msg::Req(Request::State, rtx)).is_err() {
                        return;
                    }
                    let Ok(st) = rrx.recv_timeout(Duration::from_secs(5)) else { return };
                    if ipc::wait_met(&st, &what) {
                        break format!("{{\"ok\":true,\"met\":true,\"state\":{st}}}");
                    }
                    if t0.elapsed().as_secs_f64() >= timeout_s {
                        break format!("{{\"ok\":false,\"met\":false,\"error\":\"timeout\",\"state\":{st}}}");
                    }
                    std::thread::sleep(Duration::from_millis(500));
                };
                if writeln!(w, "{reply}").is_err() {
                    return;
                }
            }
            Ok(r) => {
                let (rtx, rrx) = channel();
                if tx.send(Msg::Req(r, rtx)).is_err() {
                    return;
                }
                let Ok(reply) = rrx.recv_timeout(Duration::from_secs(5)) else { return };
                if writeln!(w, "{reply}").is_err() {
                    return;
                }
            }
        }
    }
}

/// Bind the socket, refusing if another daemon answers on it.
fn bind(path: &Path) -> Result<UnixListener, String> {
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            return Err(format!("another omarchyvrd / omarchyvr-autod is running ({})", path.display()));
        }
        let _ = std::fs::remove_file(path);
    }
    UnixListener::bind(path).map_err(|e| format!("{}: {e}", path.display()))
}

const CRASH_PATTERNS: &[&str] = &["panicked at", "SIGSEGV", "Segmentation fault", "Aborted", "core dumped", "terminate called", "fatal runtime error", "SIGABRT"];

/// systemd-coredump recorded a core of this pid (an adopted process killed by SEGV/ABRT leaves no other trace).
/// Read-only; the dump may take a moment to be registered, so it is asked twice.
fn core_dumped(pid: u32) -> bool {
    if std::env::var_os("PIMAX_NO_JOURNAL").is_some() {
        return false;
    }
    for i in 0..2 {
        if i > 0 {
            std::thread::sleep(Duration::from_millis(1500));
        }
        let out = omarchyvr_components::events::run_capture(&["coredumpctl", "-q", "--no-legend", "--no-pager", "list", &pid.to_string()], Duration::from_secs(3));
        if out.is_some_and(|o| !o.trim().is_empty()) {
            return true;
        }
    }
    false
}

/// The probe thread: a round every 2 s or on a hint.
fn probe_thread(roots: Roots, tx: Sender<Msg>, hint: std::sync::mpsc::Receiver<()>) {
    let mut seen_pids: BTreeMap<Id, u32> = BTreeMap::new();
    let mut mlog_off: u64 = 0;
    let mlog = roots.session_dir.join("monado.log");
    let mut first = true;
    loop {
        let o = omarchyvr_components::probe::Opts { deep: true, ..Default::default() };
        let obs = omarchyvr_components::probe::probe_all(&roots, &o, omarchyvr_components::model::GateLevel::Ok);
        let facts = omarchyvr_components::probe::session_facts(&roots);
        let mut captures = Vec::new();
        for (id, ob) in &obs {
            if let (Some(true), Some(pid)) = (ob.up, ob.pid) {
                if seen_pids.get(id) != Some(&pid) {
                    seen_pids.insert(id, pid);
                    let argv = omarchyvr_components::procs::cmdline(&roots.proc_, pid);
                    if !argv.is_empty() {
                        let env = omarchyvr_components::procs::environ(&roots.proc_, pid);
                        captures.push((*id, Captured { argv, env, cwd: omarchyvr_components::procs::cwd(&roots.proc_, pid) }));
                    }
                }
            }
        }
        // processes that are gone: is there a crash in their log tail?
        let mut log_crash = Vec::new();
        for s in omarchyvr_components::spec::SPECS.iter().filter(|s| s.pid.is_some()) {
            if obs.get(s.id).and_then(|o| o.up) == Some(false) && seen_pids.contains_key(s.id) {
                let stem = s.pid.unwrap().file.trim_end_matches(".pid");
                let t = omarchyvr_components::util::tail(&roots.session_dir.join(format!("{stem}.log")), 4096);
                let pid = seen_pids[s.id];
                if CRASH_PATTERNS.iter().any(|p| t.contains(p)) || core_dumped(pid) {
                    log_crash.push(s.id);
                }
                seen_pids.remove(s.id);
            }
        }
        // the Monado log: only lines written since the last round (all of it at start-up, for the gate)
        let len = std::fs::metadata(&mlog).map(|m| m.len()).unwrap_or(0);
        if len < mlog_off {
            mlog_off = 0;
        }
        let mut monado_lines = Vec::new();
        if len > mlog_off {
            let max = if first { len.min(4 << 20) } else { (len - mlog_off).min(1 << 20) };
            let t = omarchyvr_components::util::tail(&mlog, max);
            monado_lines = t.lines().filter(|l| omarchyvr_components::gpu::monado_line(l).is_some()).map(str::to_string).collect();
            mlog_off = len;
        }
        first = false;
        if tx.send(Msg::Probe(Box::new(ProbeRound { obs, facts, captures, monado_lines, log_crash }))).is_err() {
            return;
        }
        match hint.recv_timeout(Duration::from_secs(2)) {
            Ok(()) => {
                // a burst of uevents: let it settle, then re-scan once
                std::thread::sleep(Duration::from_millis(500));
                while hint.try_recv().is_ok() {}
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Run until SIGINT/SIGTERM.
pub fn run(cfg: Config, dcfg: DaemonConfig, home: PathBuf, sock: PathBuf) -> Result<(), String> {
    let stop = pimax::sys::install_term_flag();
    let listener = bind(&sock)?;
    let roots = Roots::from_env();
    let (tx, rx) = channel::<Msg>();
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for s in listener.incoming().flatten() {
                let tx = tx.clone();
                std::thread::spawn(move || serve_conn(s, tx));
            }
        });
    }
    // event sources
    let (src_tx, src_rx) = channel::<Ev>();
    let uevents = UeventSocket::open().map(|s| s.spawn(src_tx.clone())).is_ok();
    let pidwatch = PidWatch::spawn(src_tx.clone()).ok();
    let mut klog = KlogFollower::spawn(src_tx.clone(), None).ok();
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for ev in src_rx {
                if tx.send(Msg::Src(ev)).is_err() {
                    return;
                }
            }
        });
    }
    let (hint_tx, hint_rx) = channel::<()>();
    {
        let tx = tx.clone();
        let roots = roots.clone();
        std::thread::spawn(move || probe_thread(roots, tx, hint_rx));
    }
    let (reap_tx, reap_rx) = channel::<Reaped>();
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for r in reap_rx {
                if tx.send(Msg::Reaped(r)).is_err() {
                    return;
                }
            }
        });
    }
    // the reconciler
    let boot = omarchyvr_components::gpu::boot_id(&roots.proc_);
    let mut files = Files::new(&roots);
    let mut world = World::new(dcfg.desired.clone(), dcfg.budgets.clone(), dcfg.switches.clone(), omarchyvr_components::gpu::Gate::new(&boot), Default::default());
    for l in omarchyvr_components::events::kernel_log_this_boot() {
        world.kernel_line(0.0, &l);
    }
    files.load(&mut world, true);
    if files.record_start(omarchyvr_components::util::unix_now()) && world.safe_auto.is_none() {
        world.safe_auto = Some("omarchyvrd restarted more than 3 times in 5 min".into());
    }
    let mut sup = Supervisor::new(world);
    log(&format!(
        "started: auto mode {} (mode {}, preset {}); reconciler {}; uevents {}, pidfd {}, kernel log {}; socket {}",
        if cfg.dry_run { "DRY RUN" } else { "LIVE" },
        cfg.mode.name(),
        cfg.preset,
        if sup.w.observe_only() {
            match (&sup.w.safe_auto, sup.w.safe_mode) {
                (Some(r), _) => format!("SAFE MODE ({r}): observe only"),
                (None, true) => "SAFE MODE (flag file): observe only".into(),
                _ => "OBSERVE ONLY ([restart] enabled = false)".into(),
            }
        } else {
            "ACTIVE".into()
        },
        if uevents { "on" } else { "off (2 s polling)" },
        if pidwatch.is_some() { "on" } else { "off" },
        if klog.is_some() { "on" } else { "off" },
        sock.display()
    ));
    let poll = Duration::from_secs_f64(cfg.poll_s);
    let mut collector = Collector::new(Paths::from_env(), &cfg);
    let mut m = Machine::new(cfg);
    let t0 = Instant::now();
    let mut subs: Vec<Sender<String>> = Vec::new();
    let mut next_tick = Instant::now();
    let mut dry_session: Option<Kind> = None;
    let mut resume = Resume::default();
    let mut inhibit = Inhibit::default();
    let mut started_probe = false;
    let emit = |subs: &mut Vec<Sender<String>>, line: &str| subs.retain(|s| s.send(line.to_string()).is_ok());
    while !stop.load(Ordering::Relaxed) {
        let wait = next_tick.saturating_duration_since(Instant::now());
        let t = t0.elapsed().as_secs_f64();
        let mut acts = Vec::new();
        let mut reconcile_now = false;
        match rx.recv_timeout(wait) {
            Ok(Msg::Req(Request::Status, r)) => {
                let _ = r.send(ipc::status_line(&m, t));
            }
            Ok(Msg::Req(Request::State, r)) => {
                let _ = r.send(sup.snapshot(&format!(",\"autod\":{}", ipc::status_line(&m, t))));
            }
            Ok(Msg::Req(Request::Cmd(c), r)) => {
                log(&format!("command {c:?}"));
                acts = m.step(t, Input::Cmd(c));
                let _ = r.send(ipc::status_line(&m, t));
            }
            Ok(Msg::Req(Request::Supervise(c), r)) => {
                log(&format!("request {c:?}"));
                let reply = ipc::apply_supervise(&mut sup, &c, &files.safe_flag, &m.cfg.pimax_bin, &roots.session_dir);
                let _ = r.send(reply);
                reconcile_now = true;
            }
            Ok(Msg::Req(Request::Events | Request::Wait { .. }, _)) => {}
            Ok(Msg::Req(Request::Pair(slot), r)) => {
                let side = crate::pairing::side_of(slot);
                match crate::pairing::Guard::acquire() {
                    Some(_) if crate::pairing::vr_session_owner().is_some() => {
                        let who = crate::pairing::vr_session_owner().unwrap_or("VR");
                        let _ = r.send(ipc::error_line(&crate::pairing::session_refusal(who, "pair")));
                    }
                    None => {
                        let _ = r.send(ipc::error_line("a pairing attempt is already running"));
                    }
                    Some(g) => {
                        log(&format!("pairing {side}: sending StartPair once (user request)"));
                        let _ = r.send(format!("{{\"ok\":true,\"pairing\":\"started\",\"side\":{}}}", ipc::q(side)));
                        let tx = tx.clone();
                        std::thread::spawn(move || {
                            let _g = g;
                            let mut io = crate::pairing::HidIo::new();
                            let emit = |l: &str| {
                                let _ = tx.send(Msg::Event(l.to_string()));
                            };
                            let o = crate::pairing::run(slot, &mut io, &mut |s| {
                                if let Some(l) = crate::pairing::state_event(s).filter(|_| matches!(s, crate::pairing::PairState::Waiting { .. })) {
                                    emit(&l)
                                }
                            });
                            log(&format!("pairing {side}: {o:?}"));
                            emit(&crate::pairing::outcome_event(slot, &o));
                        });
                    }
                }
            }
            Ok(Msg::Event(line)) => emit(&mut subs, &line),
            Ok(Msg::Subscribe(s)) => {
                let _ = s.send(ipc::status_line(&m, t));
                subs.push(s);
            }
            Ok(Msg::Done(ok, detail)) => {
                log(&format!("action done: ok={ok} {detail}"));
                acts = m.step(t, Input::Done { ok, detail });
            }
            Ok(Msg::Probe(p)) => {
                started_probe = true;
                for (id, c) in p.captures {
                    sup.captured.insert(id, c);
                }
                for l in &p.monado_lines {
                    sup.w.monado_line(l);
                }
                if let Some(s) = resume.check_now() {
                    sup.w.resumed(t, s, 15.0);
                }
                files.load(&mut sup.w, false);
                let crashed = p.log_crash.clone();
                sup.w.observe(t, &p.obs, &p.facts, &|s| crashed.contains(&s.id));
                if let Some(pw) = &pidwatch {
                    for c in sup.w.comps.values() {
                        if let (Some(true), Some(pid)) = (c.up, c.pid) {
                            let _ = pw.add(pid);
                        }
                    }
                }
                reconcile_now = true;
            }
            Ok(Msg::Src(ev)) => match ev {
                Ev::Uevent(u) => {
                    if u.is_pimax() || u.subsystem == "drm" || u.subsystem == "video4linux" {
                        emit(&mut subs, &crate::supervisor::event_line(t, "uevent", &u.subsystem, &u.summary()));
                    }
                    let _ = hint_tx.send(());
                }
                Ev::Exit(pid) => {
                    emit(&mut subs, &crate::supervisor::event_line(t, "exit", "pid", &pid.to_string()));
                    let _ = hint_tx.send(());
                }
                Ev::Kernel(l) => {
                    let interesting = omarchyvr_components::gpu::kernel_line(&l).is_some() || omarchyvr_components::gpu::crash_line(&l).is_some() || omarchyvr_components::gpu::usb_disconnect_line(&l).is_some();
                    if interesting {
                        sup.w.kernel_line(t, &l);
                        emit(&mut subs, &crate::supervisor::event_line(t, "kernel", "klog", l.trim()));
                        let _ = hint_tx.send(());
                        reconcile_now = true;
                    }
                }
            },
            Ok(Msg::Reaped((pid, code, sig))) => {
                sup.w.exit_status.insert(pid, (code, sig));
                let _ = hint_tx.send(());
            }
            Ok(Msg::StepDone(o)) => {
                log(&format!("step done: {o:?}"));
                for l in sup.finished(&o, t) {
                    emit(&mut subs, &l);
                }
                let _ = hint_tx.send(());
            }
            Err(RecvTimeoutError::Timeout) => {
                if Instant::now() >= next_tick {
                    next_tick = Instant::now() + poll;
                    let mut s = collector.collect();
                    if m.cfg.dry_run && s.session.is_none() {
                        s.session = dry_session;
                    }
                    acts = m.step(t0.elapsed().as_secs_f64(), Input::Tick(s));
                    if let Some(k) = &mut klog {
                        if !k.alive() {
                            klog = KlogFollower::spawn(tx_src_clone(&src_tx), None).ok();
                        }
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
        // auto mode's actions
        for a in acts {
            log(&format!("{}action {a:?}", if m.cfg.dry_run { "dry-run " } else { "" }));
            emit(&mut subs, &ipc::action_line(t, &a, m.cfg.dry_run));
            if m.cfg.dry_run {
                match &a {
                    Action::Start { mode, .. } | Action::Switch { mode, .. } => dry_session = Some(if *mode == Mode::Game { Kind::Game } else { Kind::Desk }),
                    Action::Stop { .. } => dry_session = None,
                    Action::Notify(_) => {}
                }
            } else {
                // auto mode's decisions are intent edits too
                match &a {
                    Action::Start { mode, .. } | Action::Switch { mode, .. } => {
                        let pm = if *mode == Mode::Game { omarchyvr_components::model::Mode::Vr } else { omarchyvr_components::model::Mode::Desk };
                        sup.set_session(true, By::Auto, Some(pm));
                        // auto mode starts it through the launcher itself (exec): not omarchyvrd's own start
                        sup.w.intent.overrides.remove("__owned");
                    }
                    Action::Stop { reason, .. } => {
                        if reason.contains("unplugged") && !sup.w.observe_only() && sup.w.switches.core {
                            log("auto mode's unplug stop is left to omarchyvrd's unplug handling");
                            continue;
                        }
                        sup.w.set_intent_off(By::Auto, false);
                    }
                    Action::Notify(_) => {}
                }
            }
            let tx = tx.clone();
            crate::exec::execute(&m.cfg, &a, &home, Box::new(move |ok, d| { let _ = tx.send(Msg::Done(ok, d)); }), log);
        }
        for tr in &m.transitions {
            log(&format!("{} -> {} ({})", tr.from.name(), tr.to.name(), tr.why));
            emit(&mut subs, &ipc::transition_line(tr));
        }
        m.transitions.clear();
        // the reconciler
        if reconcile_now && started_probe {
            sup.w.now = t;
            let d = sup.decide();
            for e in &d.events {
                if !e.contains("\"event\":\"health\"") || e.contains("\"health\":\"down\"") || e.contains("\"health\":\"failed\"") {
                    log(e);
                }
                emit(&mut subs, e);
            }
            let wayvr_up = sup.w.c("wayvr").up == Some(true);
            for n in &d.notify {
                executor::notify(&n.text, n.critical, dcfg.notify_desktop, dcfg.notify_headset && wayvr_up);
            }
            if let Some(st) = d.run {
                let ctx = Ctx {
                    roots: roots.clone(),
                    pimax_bin: m.cfg.pimax_bin.clone(),
                    mode: sup.w.mode(),
                    argv: sup.w.facts.argv.clone(),
                    seen: sup.w.comps.iter().filter(|(_, c)| c.seen_up).map(|(id, _)| *id).collect(),
                    captured: sup.captured.get(st.id).cloned(),
                    preset: sup.w.desired.preset.clone(),
                    log,
                };
                log(&format!("executing: {}", st.line()));
                let tx = tx.clone();
                let reap = reap_tx.clone();
                std::thread::spawn(move || {
                    let o = executor::execute(&st, &ctx, &reap);
                    let _ = tx.send(Msg::StepDone(o));
                });
            }
            let session_live = sup.w.intent.on() && sup.w.c("monado").up == Some(true);
            inhibit.set(session_live && sup.w.switches.inhibit_sleep && !sup.w.observe_only(), log);
            let snap = sup.snapshot(&format!(",\"autod\":{}", ipc::status_line(&m, t)));
            files.save(&sup.w, &snap);
        }
    }
    inhibit.set(false, log);
    let _ = std::fs::remove_file(&sock);
    log("exiting (sessions are left running)");
    Ok(())
}

fn tx_src_clone(tx: &Sender<Ev>) -> Sender<Ev> {
    tx.clone()
}
