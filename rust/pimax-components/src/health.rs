//! `pimax-health [--json] [--deep] [--watch]` (also `pimaxd health`): the component table from a one-shot probe
//! of this machine. Read-only: probes, the gate latch file and the kernel log of this boot; nothing is started,
//! stopped, signalled or written. When pimaxd runs, its snapshot (`state.json`) is preferred unless `--probe`.
use crate::gpu::Gate;
use crate::state::{Switches, World};
use crate::util::Roots;

pub const USAGE: &str = "usage: pimax-health [--json] [--deep] [--probe] [--watch [--interval S]]
  the state of every component of the Pimax stack (docs/PIMAXD.md). Read-only.
  --json    the snapshot (schema v1)          --deep   also ask the passthrough/voice control sockets
  --probe   probe here even when pimaxd runs  --watch  re-print on every change (uevents, process exits, kernel lines)";

/// Build a world from one probe round. The gate comes from the latch file plus this boot's kernel log.
pub fn probe_world(r: &Roots, deep: bool) -> World {
    let boot = crate::gpu::boot_id(&r.proc_);
    let mut gate = Gate::load(&r.pimaxd_dir().join("gpu-gate"), &boot);
    for l in crate::events::kernel_log_this_boot() {
        if let Some(ev) = crate::gpu::kernel_line(&l) {
            gate.raise(ev.level(), &l);
        }
    }
    let mut w = World::new(Default::default(), Default::default(), Switches::default(), gate, Default::default());
    if let Some(i) = crate::events::read_intent(&r.pimaxd_dir().join("intent")) {
        w.intent = i;
    }
    let o = crate::probe::Opts { deep, ..Default::default() };
    let obs = crate::probe::probe_all(r, &o, w.gate.effective());
    let facts = crate::probe::session_facts(r);
    w.observe(0.0, &obs, &facts, &|_| false);
    w
}

/// The daemon's snapshot if it is fresh (< 10 s).
pub fn daemon_snapshot(r: &Roots) -> Option<String> {
    let p = r.pimaxd_dir().join("state.json");
    (crate::util::age_s(&p)? < 10.0).then(|| std::fs::read_to_string(&p).ok()).flatten()
}

fn render(r: &Roots, json: bool, deep: bool, probe: bool) -> String {
    if !probe {
        if let Some(s) = daemon_snapshot(r) {
            if json {
                return s.trim().to_string();
            }
            return format!("(from pimaxd)\n{}", table_of_snapshot(&s));
        }
    }
    let w = probe_world(r, deep);
    let p = crate::reconcile::reconcile(&w);
    if json {
        crate::snapshot::json(&w, &p, "")
    } else {
        crate::snapshot::table(&w, &p)
    }
}

/// A table from a snapshot JSON (pimaxd's view).
pub fn table_of_snapshot(s: &str) -> String {
    let Ok(v) = crate::json::parse(s.trim()) else { return s.to_string() };
    let g = |k: &str| v.s(k).unwrap_or("?").to_string();
    let i = v.get("intent");
    let mut o = format!(
        "overall: {}   session: {} (by {})   gpu gate: {}{}\n",
        g("overall"),
        i.and_then(|i| i.s("session")).unwrap_or("?"),
        i.and_then(|i| i.s("by")).unwrap_or("?"),
        g("gpu_gate"),
        if v.get("observe_only").and_then(|b| b.as_bool()) == Some(true) { "   (observe only)" } else { "" }
    );
    for c in v.get("components").map(|c| c.as_arr()).unwrap_or(&[]) {
        let health = c.s("health").unwrap_or("?");
        if health == "off" && c.get("wanted").and_then(|b| b.as_bool()) != Some(true) && c.s("kind") != Some("device") {
            continue;
        }
        let why = c.s("why").or(c.s("detail")).unwrap_or("");
        o.push_str(&format!("  {:<18} {:<9} {}\n", c.s("id").unwrap_or("?"), health, why));
    }
    if let Some(n) = v.get("next_action").filter(|n| **n != crate::json::Value::Null) {
        o.push_str(&format!("next: {} {}: {}\n", n.s("what").unwrap_or(""), n.s("id").unwrap_or(""), n.s("why").unwrap_or("")));
    }
    o
}

/// The CLI. Returns the exit code (0 ok, 1 degraded/down, 2 needs-you/needs-reboot, 64 usage).
pub fn main(args: &[String]) -> i32 {
    let mut json = false;
    let mut deep = false;
    let mut watch = false;
    let mut probe = false;
    let mut interval = 2.0;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--json" => json = true,
            "--deep" => deep = true,
            "--watch" => watch = true,
            "--probe" => probe = true,
            "--interval" => interval = it.next().and_then(|v| v.parse().ok()).unwrap_or(2.0f64).clamp(0.2, 60.0),
            "-h" | "--help" => {
                println!("{USAGE}");
                return 0;
            }
            other => {
                eprintln!("pimax-health: unknown option {other}\n{USAGE}");
                return 64;
            }
        }
    }
    let r = Roots::from_env();
    if watch {
        return crate::events::watch(&r, json, deep, probe, interval, &render);
    }
    let out = render(&r, json, deep, probe);
    println!("{out}");
    let overall = if json {
        crate::json::parse(&out).ok().and_then(|v| v.s("overall").map(str::to_string)).unwrap_or_default()
    } else {
        out.lines().find_map(|l| l.split("overall: ").nth(1)).and_then(|s| s.split_whitespace().next()).unwrap_or("").to_string()
    };
    match overall.as_str() {
        "ok" => 0,
        "degraded" | "down" => 1,
        _ => 2,
    }
}
