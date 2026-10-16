//! The state snapshot (`{"cmd":"state"}`, `$XDG_RUNTIME_DIR/pimaxd/state.json`, `pimax-health --json`) and the
//! human table. Schema v1 is frozen in docs/PIMAXD.md: fields may be added, never renamed or removed.
use crate::json::{num, q};
use crate::model::{Health, Toggle};
use crate::reconcile::{Act, Plan};
use crate::spec::SPECS;
use crate::state::World;

fn opt_num(v: Option<f64>) -> String {
    v.map_or("null".into(), num)
}

fn opt_str(v: Option<&str>) -> String {
    v.map_or("null".into(), q)
}

fn toggle_name(t: Toggle) -> String {
    match t {
        Toggle::None => "none".into(),
        Toggle::Live => "live".into(),
        Toggle::Restart(p) => format!("restart:{p}"),
    }
}

/// The snapshot as one JSON line.
pub fn json(w: &World, plan: &Plan, extra: &str) -> String {
    let h = w.health();
    let overall = w.overall(&h);
    let mut comps = Vec::new();
    for s in SPECS {
        let c = w.c(s.id);
        let hh = &h[s.id];
        let pol = w.budgets.policy(s.class);
        let until = match hh {
            Health::Starting { until } => Some(*until),
            _ => None,
        };
        let last_exit = match &c.last_exit {
            Some((t, cl, how)) => format!("{{\"class\":{},\"how\":{},\"t\":{}}}", q(cl.name()), q(how), num(*t)),
            None => "null".into(),
        };
        comps.push(format!(
            concat!(
                "{{\"id\":{},\"label\":{},\"kind\":{},\"wanted\":{},\"health\":{},\"why\":{},\"since\":{},\"until\":{},",
                "\"pid\":{},\"ready\":{},\"detail\":{},\"class\":{},\"toggle\":{},",
                "\"restarts\":{{\"used\":{},\"budget\":{},\"breaker\":{}}},\"last_exit\":{}}}"
            ),
            q(s.id),
            q(s.label),
            q(s.kind.name()),
            w.wanted(s.id),
            q(hh.name()),
            opt_str(hh.why()),
            num(c.since),
            opt_num(until),
            c.pid.filter(|_| c.up == Some(true)).map_or("null".into(), |p| p.to_string()),
            c.ready.map_or("null".into(), |b| b.to_string()),
            q(&c.detail),
            q(s.class.name()),
            q(&toggle_name(s.toggle)),
            c.restart.used(&pol, w.now),
            if pol.max > 0 { q(&crate::policy::budget_text((pol.max, pol.window_s))) } else { "null".into() },
            q(c.restart.breaker.name()),
            last_exit,
        ));
    }
    let next = match plan.next() {
        Some(st) => {
            let at = match st.act {
                Act::Wait { until } => until,
                _ => Some(w.now),
            };
            format!("{{\"what\":{},\"id\":{},\"why\":{},\"at\":{},\"level\":{}}}", q(st.act.name()), q(st.id), q(&st.why), opt_num(at), q(&format!("{:?}", st.level)))
        }
        None => "null".into(),
    };
    let overrides: Vec<String> = w.intent.overrides.iter().filter(|(k, _)| !k.starts_with("__")).map(|(k, v)| format!("{}:{v}", q(k))).collect();
    let evidence: Vec<String> = w.gate.evidence.iter().map(|e| q(e)).collect();
    let launcher = if w.facts.starting {
        "starting"
    } else if w.facts.stopping {
        "stopping"
    } else {
        "idle"
    };
    format!(
        concat!(
            "{{\"v\":1,\"t\":{},\"unix\":{},\"overall\":{},\"safe_mode\":{},\"safe_reason\":{},\"observe_only\":{},",
            "\"gpu_gate\":{},\"gpu_evidence\":[{}],\"boot_id\":{},",
            "\"intent\":{{\"session\":{},\"by\":{},\"mode\":{},\"overrides\":{{{}}}}},",
            "\"session\":{{\"mode\":{},\"launcher\":{},\"watchdog\":{}}},",
            "\"components\":[{}],\"next_action\":{},\"busy\":{}{}}}"
        ),
        num(w.now),
        num(crate::util::unix_now()),
        q(overall.name()),
        w.safe_mode || w.safe_auto.is_some(),
        opt_str(w.safe_auto.as_deref().or(if w.safe_mode { Some("safe-mode flag") } else { None })),
        w.observe_only(),
        q(w.gate.effective().name()),
        evidence.join(","),
        q(&w.gate.boot_id),
        q(w.intent.session_name()),
        q(w.intent.by.name()),
        q(w.intent.mode.name()),
        overrides.join(","),
        w.facts.mode.map_or("null".into(), |m| q(m.name())),
        q(launcher),
        w.facts.watchdog,
        comps.join(","),
        next,
        opt_str(w.busy.as_deref()),
        extra,
    )
}

/// The human table (`pimax-health`).
pub fn table(w: &World, plan: &Plan) -> String {
    let h = w.health();
    let mut o = format!(
        "overall: {}   session: {} (by {}, {})   gpu gate: {}{}\n",
        w.overall(&h).name(),
        w.intent.session_name(),
        w.intent.by.name(),
        w.facts.mode.unwrap_or(w.intent.mode).name(),
        w.gate.effective().name(),
        if w.observe_only() { "   (observe only)" } else { "" }
    );
    for id in crate::graph::start_order() {
        let s = crate::spec::spec(id).unwrap();
        let c = w.c(id);
        let hh = &h[id];
        if *hh == Health::Off && !w.wanted(id) && s.kind != crate::model::Kind::Device {
            continue;
        }
        let dot = match hh {
            Health::Ok => "ok  ",
            Health::Off => "off ",
            Health::Starting { .. } => "... ",
            Health::Degraded(_) => "warn",
            Health::Unknown => "?   ",
            _ => "FAIL",
        };
        let why = hh.why().map(str::to_string).unwrap_or_else(|| c.detail.clone());
        o.push_str(&format!("  [{dot}] {:<18} {:<9} {}\n", id, hh.name(), why));
    }
    if let Some(st) = plan.next() {
        o.push_str(&format!("next: {}\n", st.line()));
    }
    for e in &w.gate.evidence {
        o.push_str(&format!("gpu evidence: {e}\n"));
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse;

    #[test]
    fn the_snapshot_is_valid_json_with_every_component() {
        let w = World::new(Default::default(), Default::default(), Default::default(), crate::gpu::Gate::new("b\"1"), Default::default());
        let p = crate::reconcile::reconcile(&w);
        let v = parse(&json(&w, &p, ",\"autod\":null")).unwrap();
        assert_eq!(v.get("v").unwrap().as_f64(), Some(1.0));
        assert_eq!(v.s("overall"), Some("ok"));
        assert_eq!(v.get("components").unwrap().as_arr().len(), SPECS.len());
        assert_eq!(v.get("intent").unwrap().s("session"), Some("unset"));
        assert_eq!(v.s("boot_id"), Some("b\"1"));
        assert!(table(&w, &p).starts_with("overall: ok"));
    }
}
