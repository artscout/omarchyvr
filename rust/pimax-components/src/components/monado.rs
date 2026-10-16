//! monado-service (moved from the launcher's `pimax-tools/src/vr/session.rs` in P3): stopping it by pid. SIGINT once
//! (its handler is single-shot), wait the grace, and SIGKILL only when the caller is an explicit user stop
//! (`kill = true`); every automatic stop leaves a hung service alive ([`StopOutcome::Hung`]) because a forced kill
//! during a stuck flip wedges the display until a reboot (resilience plan G1). The start (preflight, runtime link,
//! environment, readiness) is still the launcher's `session::start`; pimaxd falls back to it through the CLI.
use super::proc::{Procs, Signal};
use super::proc::Clock;
use crate::component::{Component, Env};
use crate::spec::Spec;
use std::time::Duration;

/// Poll interval while waiting for the exit.
pub const POLL: Duration = Duration::from_millis(100);

/// How stopping one process went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopOutcome {
    /// Gone after SIGINT.
    Graceful(Duration),
    /// Needed SIGKILL.
    Killed,
    /// Still there after SIGKILL (uninterruptible sleep, probably the GPU).
    Stuck,
    /// Still there after the grace period, and SIGKILL was withheld (automatic stops never kill Monado).
    Hung,
    /// The signal could not be sent (not ESRCH: for example EPERM).
    CannotSignal(String),
}

/// Seconds after which a missing SIGINT handler is taken as "a SIGINT was already delivered" rather than "not installed yet".
const HANDLER_SETTLE_SECS: f64 = 5.0;

/// True if the service already consumed a SIGINT: Monado installs its shutdown handler with SA_RESETHAND, so a second
/// SIGINT would hit the default action and kill it in the middle of its teardown (the display engine hazard).
pub fn sigint_already_consumed(pid: u32, procs: &dyn Procs) -> bool {
    procs.catches_sigint(pid) == Some(false)
        && procs.age_secs(pid).is_some_and(|a| a > HANDLER_SETTLE_SECS)
}

/// SIGINT (not when one was already delivered), wait up to `grace`, then SIGKILL with a warning through `warn`.
pub fn stop_pid(
    pid: u32,
    procs: &dyn Procs,
    clock: &dyn Clock,
    grace: Duration,
    info: &mut dyn FnMut(&str),
    warn: &mut dyn FnMut(&str),
) -> StopOutcome {
    stop_pid_with(pid, procs, clock, grace, true, info, warn)
}

/// [`stop_pid`]; with `kill = false` (every automatic stop: the watchdog, auto mode) a service still alive after the
/// grace is left running ([`StopOutcome::Hung`]) and its hang evidence goes to `warn` instead of a SIGKILL: a forced
/// kill during a stuck flip wedges the display until a reboot (resilience plan G1).
pub fn stop_pid_with(
    pid: u32,
    procs: &dyn Procs,
    clock: &dyn Clock,
    grace: Duration,
    kill: bool,
    info: &mut dyn FnMut(&str),
    warn: &mut dyn FnMut(&str),
) -> StopOutcome {
    let t0 = clock.now();
    if sigint_already_consumed(pid, procs) {
        info(&format!(
            "pid {pid} already received a SIGINT and is shutting down; not sending another (its handler is single-shot and a second SIGINT would kill it half way)"
        ));
    } else {
        match procs.signal(pid, Signal::Int) {
            Ok(()) => {}
            Err(e) if e.raw_os_error() == Some(libc::ESRCH) => {
                return StopOutcome::Graceful(Duration::ZERO)
            }
            Err(e) => return StopOutcome::CannotSignal(format!("SIGINT to pid {pid}: {e}")),
        }
        if !procs.is_alive(pid) {
            return StopOutcome::Graceful(Duration::ZERO);
        }
    }
    while clock.now().saturating_sub(t0) < grace {
        clock.sleep(POLL);
        if !procs.is_alive(pid) {
            return StopOutcome::Graceful(clock.now().saturating_sub(t0));
        }
    }
    // what was it doing? (bounded; never blocks the kill for long)
    let evidence = procs.capture_hang(pid);
    if !kill {
        warn(&format!(
            "monado-service (pid {pid}) did not exit {:.0} s after SIGINT. NOT killed (an automatic stop never sends \
             SIGKILL: it can leave the NVIDIA display engine stuck). Reboot before the next session, or stop it yourself \
             with `pimax vr stop`{}",
            grace.as_secs_f64(),
            evidence.map(|e| format!(". {e}")).unwrap_or_default()
        ));
        return StopOutcome::Hung;
    }
    warn(&format!(
        "monado-service (pid {pid}) did not exit {:.0} s after SIGINT: sending SIGKILL. A forced kill can leave the \
         NVIDIA display engine stuck; if the headset stays black check journalctl -k -b for 'Idling display engine' \
         and reboot{}",
        grace.as_secs_f64(),
        evidence.map(|e| format!(". {e}")).unwrap_or_default()
    ));
    match procs.signal(pid, Signal::Kill) {
        Ok(()) => {}
        Err(e) if e.raw_os_error() == Some(libc::ESRCH) => return StopOutcome::Killed,
        Err(e) => return StopOutcome::CannotSignal(format!("SIGKILL to pid {pid}: {e}")),
    }
    let t1 = clock.now();
    while clock.now().saturating_sub(t1) < Duration::from_secs(3) {
        if !procs.is_alive(pid) {
            return StopOutcome::Killed;
        }
        clock.sleep(POLL);
    }
    if procs.is_alive(pid) {
        StopOutcome::Stuck
    } else {
        StopOutcome::Killed
    }
}

/// The `monado` component: its stop never SIGKILLs.
pub struct Monado(pub &'static Spec);

impl Component for Monado {
    fn spec(&self) -> &'static Spec {
        self.0
    }
    fn stop(&self, env: &dyn Env, grace: Duration) -> crate::component::StopOutcome {
        let r = env.roots();
        let Some(ps) = self.0.pid else { return crate::component::StopOutcome::Stopped };
        let procs = super::proc::RealProcs::new(&r.proc_);
        let pf = crate::procs::pid_path(r, &ps);
        let Some(pid) = super::proc::read_pid(&pf) else { return crate::component::StopOutcome::Stopped };
        if !procs.is_alive(pid) || procs.comm(pid).as_deref() != Some(ps.comm) {
            return crate::component::StopOutcome::Stopped;
        }
        let clock = super::proc::RealClock::new();
        let out = stop_pid_with(pid, &procs, &clock, grace, false, &mut |m| env.log(m), &mut |m| env.log(m));
        map_outcome(pid, out)
    }
}

/// The launcher's outcome as the component model's.
pub fn map_outcome(pid: u32, out: StopOutcome) -> crate::component::StopOutcome {
    match out {
        StopOutcome::Graceful(_) => crate::component::StopOutcome::Stopped,
        StopOutcome::Hung | StopOutcome::Stuck => crate::component::StopOutcome::Hung { pid },
        StopOutcome::Killed => crate::component::StopOutcome::Failed(format!("monado-service pid {pid} was SIGKILLed")),
        StopOutcome::CannotSignal(m) => crate::component::StopOutcome::Failed(m),
    }
}

#[cfg(test)]
mod tests {
    use super::super::proc::fake::*;
    use super::*;

    #[test]
    fn an_automatic_stop_never_sends_sigkill() {
        let clock = FakeClock::new();
        let procs = FakeProcs::new(&clock);
        procs.add(42, "monado-service", None);
        let mut warn = vec![];
        let out = stop_pid_with(42, &procs, &clock, Duration::from_secs(3), false, &mut |_| {}, &mut |m| warn.push(m.to_string()));
        assert_eq!(out, StopOutcome::Hung);
        assert_eq!(*procs.log.borrow(), vec![(42, Signal::Int)]);
        assert!(warn[0].contains("NOT killed"));
        assert_eq!(map_outcome(42, out), crate::component::StopOutcome::Hung { pid: 42 });
        // an explicit user stop may escalate
        let procs = FakeProcs::new(&clock);
        procs.add(43, "monado-service", None);
        let out = stop_pid(43, &procs, &clock, Duration::from_secs(1), &mut |_| {}, &mut |_| {});
        assert_eq!(*procs.log.borrow(), vec![(43, Signal::Int), (43, Signal::Kill)]);
        assert_eq!(out, StopOutcome::Stuck);
    }
}
