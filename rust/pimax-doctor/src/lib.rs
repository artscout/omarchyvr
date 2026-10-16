//! Health checks for the Pimax Dream Air setup. Everything is read-only: the checks read sysfs/procfs, run a few
//! read-only tools (`systemctl is-active`, `getcap`, `journalctl -k`, `wayland-info`, `patch --dry-run`) and never touch
//! the headset, the display (no hotplug writes) or Monado. Fixes that need root are only *described* ([`Fix`]).
//!
//! The machine is reached through the [`Host`] trait so tests use an in-memory fake ([`fake::FakeHost`]) and the real
//! host can be pointed at fake `/sys`, `/proc` and `/dev` trees ([`host::Roots`]).
//!
//! Integration note for `pimax-tools`: its `vr start` preflight can call [`run_checks`] and refuse on any
//! [`Level::Fail`] (or just the ids it cares about: `kernel-log`, `connector`, `keepalive`, `setcap`, `stale-service`).
//! `pimax-tools` must not be edited from here; it only needs `pimax-doctor = { path = "../pimax-doctor" }`.
pub mod checks;
pub mod fake;
pub mod host;
pub mod info;
pub mod json;
pub mod report;

pub use checks::{run_checks, Opts};
pub use host::{CmdOut, Host, RealHost, Roots};
pub use report::{Check, Fix, Level, Report};

/// Vendor id of the Pimax headset family.
pub const PIMAX_VID: u16 = 0x34a4;
/// Dream Air (SLAM) interface device.
pub const DREAM_AIR_PID: u16 = 0x0044;
/// Tobii eye chip.
pub const TOBII_VID: u16 = 0x2104;
/// Tobii eye chip product id.
pub const TOBII_PID: u16 = 0x0561;
