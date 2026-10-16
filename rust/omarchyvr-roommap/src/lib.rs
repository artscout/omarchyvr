//! Persistent room map for the Pimax Dream Air (design and numbers: `analysis/room-map.md`).
//!
//! Basalt (the VIO inside Monado) starts a new world at every start and cannot save or load a map. This crate keeps
//! a map of the room between sessions and recognises it again, so the world origin comes back to the same real
//! place:
//!
//! * [`rec`]: offline input (EuRoC recording + Basalt trajectory of the same session).
//! * [`pipeline`]: map building (stereo ORB keyframes from `omarchyvr-room`) and per-frame matching.
//! * [`loopclose`]: loop closure inside the mapping session (4-DoF pose graph over the keyframes).
//! * [`refine`]: multi-view re-triangulation of the map points over the mapping session.
//! * [`align`]: relocalisation pooled over many frames (one gravity-aware 4-DoF `room_from_slam` for all of them).
//! * [`store`]: map directory, results, and the Monado world offset (the `PIMAX_ANCHOR=1` hook of `omarchyvr-anchor`).
//! * [`floor`]: floor detection (map / stereo points, controller touch, manual), storage, continuous lock.
//! * [`export`]: the map as JSON and as one offline HTML5 viewer page.
//! * [`setup`]: room setup: play area (boundary), centre and forward, SteamVR collision walls, guided steps.
//! * [`light`]: image quality for mapping (brightness, ORB corners, stereo points) and the user's advice.
//! * [`live`]: the same from `pimax-cam broker` frames + Monado's head-pose ring.
#![allow(clippy::needless_range_loop, clippy::type_complexity)]

pub mod align;
pub mod export;
pub mod floor;
pub mod light;
pub mod live;
pub mod loopclose;
pub mod pipeline;
pub mod rec;
pub mod setup;
pub mod refine;
pub mod store;

#[cfg(test)]
mod tests;
