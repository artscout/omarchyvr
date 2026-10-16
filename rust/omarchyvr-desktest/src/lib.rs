//! Offline analysis helpers for the VR-desktop Phase 0 tests (see `analysis/overnight/desk.md`).
//!
//! * [`metrics`] decodes Monado's `XRT_METRICS_FILE` (protobuf stream written by nanopb) and summarises compositor GPU
//!   time and frame cadence for each *set of client sessions that contributed layers* (e.g. game alone, game + overlay).
//! * [`dump`] reads the raw target image written by `XRT_COMPOSITOR_DUMP_DIR` (A2B10G10R10 or 8-bit RGBA/BGRA),
//!   crops/scales it and writes PNG ([`png`]).
//! * [`sharp`] measures how sharp a rendered test chart is: line-pair modulation and 10-90 % edge rise.
#![forbid(unsafe_code)]

pub mod dump;
pub mod metrics;
pub mod png;
pub mod sharp;
