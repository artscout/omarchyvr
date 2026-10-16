//! The Pimax stack's component model (docs/RESILIENCE-PLAN.md, analysis/resilience-plan.html).
//!
//! * [`model`]: ids, kinds, health, toggles, ladders, grace, desired state and intent;
//! * [`spec`]: the component table; [`graph`]: dependency order; [`component`]: the [`Component`] trait and
//!   [`components`]: one module per component (Lane A adds start/stop there);
//! * [`policy`]: budgets, backoff, breaker, flapping, exit classes; [`gpu`]: the GPU safety gate;
//! * [`probe`], [`usb`], [`procs`]: read-only probes; [`state`]: the actual state ([`state::World`]);
//! * [`reconcile`]: the pure plan; [`snapshot`]: the state JSON (schema in docs/PIMAXD.md) and the health table;
//! * [`events`]: uevent, pidfd, kernel log follower, resume detection; [`health`]: `pimax-health`.
//!
//! std threads, libc only, no serde, no async.
pub mod component;
pub mod components;
pub mod events;
pub mod gpu;
pub mod graph;
pub mod health;
pub mod intent;
pub mod json;
pub mod model;
pub mod policy;
pub mod probe;
pub mod procs;
pub mod reconcile;
pub mod snapshot;
pub mod spec;
pub mod state;
pub mod usb;
pub mod util;

pub use component::{Component, Env, RealEnv, Started, StopOutcome};
pub use model::{Health, Id, Kind, Overall};
