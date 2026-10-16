//! The dependency graph over [`crate::spec::SPECS`]: topological start order, reverse stop order, dependents.
//! Pure. A cycle is a programming error (tested), so the functions return `Result` only where tables are built.
use crate::model::{Desired, Id};
use crate::spec::{Spec, SPECS};

/// Ids in dependency order (every dependency before its dependents), ties broken by table order, so the start
/// order is stable. Soft dependencies order too (a soft dep starts first when both start). Errors on a cycle.
pub fn topo(specs: &[Spec]) -> Result<Vec<Id>, String> {
    let n = specs.len();
    let idx = |id: &str| specs.iter().position(|s| s.id == id);
    let mut indeg = vec![0usize; n];
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, s) in specs.iter().enumerate() {
        for d in s.deps {
            let j = idx(d.id).ok_or_else(|| format!("{} depends on unknown {}", s.id, d.id))?;
            out[j].push(i);
            indeg[i] += 1;
        }
    }
    let mut ready: Vec<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
    let mut order = Vec::with_capacity(n);
    while !ready.is_empty() {
        ready.sort_unstable();
        let i = ready.remove(0);
        order.push(specs[i].id);
        for &k in &out[i] {
            indeg[k] -= 1;
            if indeg[k] == 0 {
                ready.push(k);
            }
        }
    }
    if order.len() != n {
        let stuck: Vec<_> = (0..n).filter(|&i| indeg[i] > 0).map(|i| specs[i].id).collect();
        return Err(format!("dependency cycle among {stuck:?}"));
    }
    Ok(order)
}

/// The start order of the real table.
pub fn start_order() -> Vec<Id> {
    topo(SPECS).expect("the component table has no cycle (tested)")
}

/// The stop order: the start order reversed (leaves first, the broker after Monado).
pub fn stop_order() -> Vec<Id> {
    let mut v = start_order();
    v.reverse();
    v
}

/// Hard dependencies of `id` that apply to this desired state.
pub fn hard_deps(id: &str, d: &Desired) -> Vec<Id> {
    crate::spec::spec(id).map_or(vec![], |s| s.deps.iter().filter(|x| x.hard && x.active(d)).map(|x| x.id).collect())
}

/// Every component that (transitively) hard-depends on `id` for this desired state, in start order.
pub fn dependents(id: &str, d: &Desired) -> Vec<Id> {
    let mut set: Vec<Id> = Vec::new();
    for c in start_order() {
        let deps = hard_deps(c, d);
        if deps.iter().any(|x| *x == id || set.contains(x)) {
            set.push(c);
        }
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Dep, Kind, Toggle};
    use crate::spec::spec;

    #[test]
    fn real_table_orders_deps_first() {
        let o = start_order();
        let pos = |id: &str| o.iter().position(|x| *x == id).unwrap();
        for s in SPECS {
            for d in s.deps {
                assert!(pos(d.id) < pos(s.id), "{} before {}", d.id, s.id);
            }
        }
        assert!(pos("broker") < pos("monado"));
        assert!(pos("monado") < pos("wayvr"));
        assert!(pos("wayvr") < pos("perf"));
        let st = stop_order();
        let sp = |id: &str| st.iter().position(|x| *x == id).unwrap();
        assert!(sp("wayvr") < sp("monado") && sp("monado") < sp("broker"));
        assert!(sp("voice.captions") < sp("voice.listen"));
    }

    #[test]
    fn a_cycle_is_an_error() {
        let base = *spec("perf").unwrap();
        static A: [Dep; 1] = [Dep::hard("b")];
        static B: [Dep; 1] = [Dep::hard("a")];
        let t = [Spec { id: "a", deps: &A, kind: Kind::Process, toggle: Toggle::Live, ..base }, Spec { id: "b", deps: &B, ..base }];
        assert!(topo(&t).unwrap_err().contains("cycle"));
    }

    #[test]
    fn dependents_follow_the_desired_state() {
        let d = Desired::default();
        let cam = dependents("usb.cam", &d);
        for id in ["broker", "monado", "wayvr", "passthrough", "ctrltrack"] {
            assert!(cam.contains(&id), "{id} {cam:?}");
        }
        // without 6DoF Monado does not need the broker, so a camera drop leaves Monado and WayVR alone
        let no6 = Desired { sixdof: false, ..Desired::default() };
        let cam = dependents("usb.cam", &no6);
        assert!(cam.contains(&"passthrough") && !cam.contains(&"monado") && !cam.contains(&"wayvr"), "{cam:?}");
        assert!(dependents("voice.listen", &d).contains(&"voice.act"));
        assert!(!dependents("ai.server", &d).contains(&"voice.act"), "soft deps do not block");
    }
}
