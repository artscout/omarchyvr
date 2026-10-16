//! The [`Component`] trait (plan section 6.2). One implementation per component lives in
//! [`crate::components`]; the table row ([`crate::spec::Spec`]) carries the static parts, so an implementation only
//! has to provide what it does (start, stop, live toggles) and may refine its probe.
use crate::model::{Dep, Desired, Grace, Id, Intent, Kind, Rung, Toggle};
use crate::probe::Obs;
use crate::spec::Spec;
use crate::util::Roots;
use std::time::Duration;

/// What a component needs from the outside world: roots, a clock, a log. Real and fake implementations.
pub trait Env {
    fn roots(&self) -> &Roots;
    /// Seconds on a monotonic clock.
    fn now(&self) -> f64;
    fn log(&self, msg: &str);
}

/// The real environment.
pub struct RealEnv {
    pub roots: Roots,
    t0: std::time::Instant,
}

impl RealEnv {
    pub fn new(roots: Roots) -> RealEnv {
        RealEnv { roots, t0: std::time::Instant::now() }
    }
}

impl Env for RealEnv {
    fn roots(&self) -> &Roots {
        &self.roots
    }
    fn now(&self) -> f64 {
        self.t0.elapsed().as_secs_f64()
    }
    fn log(&self, msg: &str) {
        eprintln!("{msg}");
    }
}

/// Result of a start: the pid and when it must be ready.
#[derive(Debug, Clone, PartialEq)]
pub struct Started {
    pub pid: Option<u32>,
    /// Ready within this many seconds (the readiness probe decides).
    pub ready_within_s: f64,
}

/// Result of a stop.
#[derive(Debug, Clone, PartialEq)]
pub enum StopOutcome {
    /// It exited (or was not running).
    Stopped,
    /// Still alive after the grace. For Monado this is never followed by SIGKILL: the gate goes to needs-reboot.
    Hung { pid: u32 },
    Failed(String),
}

/// A component (plan section 6.2).
pub trait Component: Send + Sync {
    /// The table row.
    fn spec(&self) -> &'static Spec;

    fn id(&self) -> Id {
        self.spec().id
    }
    fn kind(&self) -> Kind {
        self.spec().kind
    }
    fn deps(&self) -> &'static [Dep] {
        self.spec().deps
    }
    /// Part of "everything on" for this desired state and intent (the daemon's [`crate::state::World::wanted`] adds
    /// adoption rules on top).
    fn wanted(&self, d: &Desired, i: &Intent) -> bool {
        let s = self.spec();
        if let Some(v) = i.overrides.get(s.id) {
            return *v;
        }
        match s.scope {
            crate::spec::Scope::Session => i.on() && (s.rule)(d),
            _ => (s.rule)(d),
        }
    }
    fn toggle(&self) -> Toggle {
        self.spec().toggle
    }
    fn ladder(&self) -> &'static [Rung] {
        self.spec().ladder
    }
    fn grace(&self) -> Grace {
        self.spec().grace
    }
    /// Read-only, bounded in time.
    fn probe(&self, env: &dyn Env) -> Obs {
        let r = env.roots();
        crate::probe::probe_all(r, &crate::probe::Opts::default(), crate::model::GateLevel::Ok).remove(self.id()).unwrap_or_default()
    }
    /// Start it. The default says it cannot (pimaxd then uses the launcher CLI or respawns the captured argv).
    fn start(&self, _env: &dyn Env, _d: &Desired) -> Result<Started, String> {
        Err(format!("{}: no start in this component module yet", self.id()))
    }
    /// Stop it within `grace`. Never SIGKILLs Monado.
    fn stop(&self, _env: &dyn Env, _grace: Duration) -> StopOutcome {
        StopOutcome::Failed(format!("{}: no stop in this component module yet", self.id()))
    }
    /// Apply a live toggle; None = needs a restart (never applied automatically).
    fn set_live(&self, _env: &dyn Env, _on: bool) -> Option<Result<(), String>> {
        None
    }
}
