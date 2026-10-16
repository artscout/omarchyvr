//! Room memory for 6DoF tracking of the Pimax Dream Air (design: `analysis/research/6dof-head-tracking.md`,
//! "Room memory"). Basalt (the VIO) keeps no map and starts a new world every session; this crate adds the
//! layer that makes the room come back where it was:
//!
//! * [`orb`]: ORB features (FAST + orientation + steered BRIEF) on the 640x480 fisheye tiles.
//! * [`stereo`]: the camera rig from the factory calibration, bearings, stereo triangulation (lower pair).
//! * [`map`]: the room map (stereo keyframes in room coordinates, anchors), file format, relocalisation.
//! * [`pnp`]: gravity-aware 4-DoF absolute pose (yaw + translation) for the multi-camera rig, RANSAC.
//! * [`session`]: one session against a map: relocalise at start, add keyframes, refine, save at the end.
//! * [`synth`]: a synthetic textured room seen through the real KB4 cameras (tests, `pimax-room demo`).
//! * [`pngread`]: minimal grey PNG reader for EuRoC recordings.
#![warn(missing_docs)]

pub mod map;
pub mod orb;
pub mod pngread;
pub mod pnp;
pub mod session;
pub mod stereo;
pub mod synth;

#[cfg(test)]
mod tests;
