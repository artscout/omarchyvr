//! `omarchyvr-voice-hud`: live voice-to-text captions for the Pimax Dream Air as an `XR_EXTX_overlay` OpenXR client.
//!
//! Everything except [`xr`] runs and is unit-tested without a headset or an OpenXR runtime: the protocol parser,
//! the socket client, the display model, the text layout and rasteriser, the placement maths, the PNG writer, the
//! `preview` and `dump` modes.

pub mod cli;
pub mod client;
pub mod color;
pub mod demo;
pub mod display;
pub mod dump;
pub mod font;
pub mod hud;
pub mod log;
pub mod placement;
pub mod png;
pub mod preview;
pub mod probe;
pub mod protocol;
pub mod render;
pub mod runner;
pub mod sig;
pub mod xr;
