//! omarchyvrd: the Pimax stack's owner (docs/PIMAXD.md): auto mode (docs/AUTO-MODE.md) plus the component reconciler
//! ([`supervisor`], [`executor`], [`scenario`], [`dconfig`]) over the `omarchyvr-components` crate.
//!
//! * [`machine`]: the pure state machine (Unplugged -> Plugged -> Worn -> Active(desk|game) -> IdleDim -> Sleep).
//! * [`sources`]: read-only signals (USB sysfs, /proc, Monado perf stats, a short hidraw IMU/controller burst).
//! * [`exec`]: actions -> `omarchyvr desk|vr start|stop`, `omarchyvr-roommap relocalize` (dry run by default).
//! * [`ipc`] + [`client`]: the JSON-lines control socket (used by `omarchyvr auto` and the GUI).
//! * [`sim`]: scripted timelines through the real machine; [`motion`]: IMU wear classifier and EuRoC replay.
//! * [`visor`]: one-shot 0x2c probe (live test only); [`pairing`]: controller pairing (StartPair only, approved 2026-10-14).
pub mod cli;
pub mod client;
pub mod config;
pub mod daemon;
pub mod dconfig;
pub mod exec;
pub mod executor;
pub mod ipc;
pub mod machine;
pub mod motion;
pub mod pairing;
pub mod scenario;
pub mod sim;
pub mod sources;
pub mod supervisor;
pub mod unpair;
pub mod visor;
