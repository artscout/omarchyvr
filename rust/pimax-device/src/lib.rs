//! Headset modules, the HID allowlist, system checks and onboarding state for the Pimax-on-Linux stack.
//!
//! Design: docs/ARCHITECTURE.md. This crate never opens a device, runs a command or touches system config:
//! modules build reports, [`hid::Gate`] checks them against the module's allowlist before writing to a writer the
//! caller opened, checks read through `pimax_doctor::Host`, and fixes are rendered for the user to run.
//!
//! Nothing outside `devices/` assumes the vendor is Pimax: a module is identified by its descriptor's USB ids.

pub mod autofix;
pub mod check;
pub mod checks;
pub mod descriptor;
pub mod devices;
pub mod hid;
pub mod module;
pub mod onboarding;
pub mod toml_lite;

pub use check::{Area, Check, Ctx, Finding, Fix, FixRef, Severity, CATALOGUE};
pub use descriptor::{Capabilities, Descriptor, Status, Support};
pub use hid::{Allowlist, Gate, Refused};
pub use module::{all, by_id, detect, Detected, Hand, HeadsetModule};
