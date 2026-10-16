//! `omarchyvr-hud`: a basic in-headset screen (large text lines and a progress bar on a panel a short distance in
//! front of the wearer), reusable by tools and the GUI.
//!
//! * [`screen`] is the pure part: [`Screen`] / [`Line`] and the CPU rendering to an RGB image (and to PNG).
//! * [`xr`] is the OpenXR (Vulkan) part: [`Hud`] owns the session and draws the screen as a quad layer;
//!   [`HudHandle`] is a cloneable, thread-safe handle to change what it shows.
//!
//! ```no_run
//! let mut hud = omarchyvr_hud::Hud::new(omarchyvr_hud::Opts::default()).unwrap();
//! let h = hud.handle();
//! h.set_lines(&["Look straight ahead", "3"]);
//! h.set_progress(Some(0.5));
//! while hud.frame().unwrap() != omarchyvr_hud::FrameResult::Exit {}
//! ```
//! Only one main OpenXR app can own the display: use [`Opts::overlay`] (`XR_EXTX_overlay`) when WayVR runs.

#![allow(clippy::chunks_exact_to_as_chunks)]

pub mod runtime;
pub mod screen;
pub mod xr;

pub use screen::{Line, Screen};
pub use xr::{FrameResult, HandInput, Hud, HudHandle, Input, Opts, Placement, Sink};
