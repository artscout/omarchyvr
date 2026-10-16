//! `omarchyvr-record run`, the recorder (docs/RECORD.md): a leaf that reads Monado's recorder tap. Started on demand
//! (`omarchyvr record start`, the app, voice); the launcher stops it first (before any other client) so the mp4 is
//! finished while Monado still runs. Its start needs the recorder's settings, so only the stop lives here.
use super::proc::{stop_client, Clock, ClientStop, Procs};
use std::path::Path;
use std::time::Duration;

/// The recorder finishes its file within this (normally well under a second).
pub const STOP_GRACE: Duration = Duration::from_secs(25);
/// comm of `omarchyvr-record`.
pub const COMM: &str = "omarchyvr-recor"; // the kernel cuts comm at 15 characters

/// The launcher's first stop step: SIGINT the recorder by its pid file (checked by comm), wait for the file to be
/// finished, remove the pid file. A recorder that is not running is fine; one that is still finishing after the
/// grace is left alone (it never blocks Monado: it only reads shared memory).
pub fn stop_first(session_dir: &Path, procs: &dyn Procs, clock: &dyn Clock, info: &mut dyn FnMut(&str)) -> ClientStop {
    let pf = session_dir.join("record.pid");
    if !pf.exists() {
        return ClientStop::NotRunning;
    }
    let r = stop_client("record", &pf, COMM, procs, clock, STOP_GRACE, info);
    if matches!(r, ClientStop::NotRunning | ClientStop::Exited) {
        let _ = std::fs::remove_file(&pf);
    }
    r
}

super::proc::client_component!(Record);

#[cfg(test)]
mod tests {
    use super::super::proc::fake::{FakeClock, FakeProcs};
    use super::super::proc::Signal;
    use super::*;

    #[test]
    fn stops_the_recorder_by_pid_with_sigint_only() {
        let d = crate::util::TempDir::new("record-stop-first");
        let clock = FakeClock::new();
        let procs = FakeProcs::new(&clock);
        assert_eq!(stop_first(d.path(), &procs, &clock, &mut |_| {}), ClientStop::NotRunning);
        std::fs::write(d.path().join("record.pid"), "77\n").unwrap();
        procs.add(77, COMM, None);
        procs.dies_on(77, Signal::Int, 2);
        assert_eq!(stop_first(d.path(), &procs, &clock, &mut |_| {}), ClientStop::Exited);
        assert_eq!(*procs.log.borrow(), vec![(77, Signal::Int)]);
        assert!(!d.path().join("record.pid").exists());
    }

    #[test]
    fn a_reused_pid_is_not_touched() {
        let d = crate::util::TempDir::new("record-stop-reused");
        let clock = FakeClock::new();
        let procs = FakeProcs::new(&clock);
        std::fs::write(d.path().join("record.pid"), "78\n").unwrap();
        procs.add(78, "bash", None);
        assert_eq!(stop_first(d.path(), &procs, &clock, &mut |_| {}), ClientStop::NotRunning);
        assert!(procs.log.borrow().is_empty());
    }

    #[test]
    fn the_component_is_an_on_demand_leaf_after_monado() {
        let s = crate::spec::spec("record").unwrap();
        assert_eq!(s.class, crate::model::Class::Leaf);
        assert!(!(s.rule)(&crate::model::Desired::default()), "never part of everything on");
        let d = crate::model::Desired::default();
        let order = crate::graph::stop_order();
        let pos = |id: &str| order.iter().position(|x| *x == id).unwrap();
        assert!(pos("record") < pos("monado"));
        assert!(crate::graph::hard_deps("record", &d).contains(&"monado"));
        assert_eq!(crate::spec::toggle_ids("record"), &["record"]);
    }
}
