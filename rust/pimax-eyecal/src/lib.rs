//! `pimax-eyecal`: eye-tracking calibration and accuracy test for the Pimax Dream Air under Monado.
//!
//! The pure parts: target [`layout`], window analysis ([`collect`]), the procedure state machine ([`procedure`]),
//! results ([`report`]), a simulated wearer ([`sim`]) and what the targets look like ([`marks`]). The OpenXR /
//! Vulkan application is in [`xrapp`] and [`gpu`]; the model itself (per-eye maps, combination, filter) lives in
//! `pimax_eye::calib` so that the service applies exactly what was fitted here.

pub mod collect;
pub mod gpu;
pub mod layout;
pub mod procedure;
pub mod refit;
pub mod report;
pub mod sim;
pub mod xrapp;
