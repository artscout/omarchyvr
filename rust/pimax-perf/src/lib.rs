//! Performance readout for the right wrist panel (`perf`, defined by the launcher's `gui/perf.xml`).
//!
//! Monado (branch `desk`, `PIMAX_PERF_FILE`) writes a small key=value text file every 0.5 s; [`parse`] reads it,
//! [`Tracker`] keeps the last few seconds of missed frames and turns it all into a [`View`] (four coloured lines),
//! and [`wayvr`] sends that to the panel with `wayvrctl batch`. [`timeline`] writes one frame-health line every 5 s
//! to stderr (the session's `perf.log`) so dropped frames can be lined up with other events afterwards.

pub mod stats;
pub mod timeline;
pub mod view;
pub mod wayvr;

pub use stats::{parse, App, Stats};
pub use timeline::Timeline;
pub use view::{level, Level, Tracker, View};
