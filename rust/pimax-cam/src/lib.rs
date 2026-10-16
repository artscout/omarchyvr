//! Pimax Dream Air tracking-camera pipeline (UVC `34a4:0014`).
//!
//! * [`frame`]: frame layout, metadata (frame kind, µs timestamp, counter, exposure/gain, per-tile row offset),
//!   tile split.
//! * [`v4l2`]: raw-ioctl V4L2 mmap capture; [`record`]: `.pxcam` recording and replay; [`source`]: both behind
//!   one trait; [`camera`]: the pipeline (stats, drops, clock, recording).
//! * [`broker`]: one process streams the camera, any number of local processes read every frame (shared-memory
//!   ring; source spec `broker`), so passthrough, SLAM and hand / controller tracking can run at once.
//! * [`resilient`]: live sources reopen the camera / reconnect to the broker after a drop or a stall.
//! * [`calib`]: factory calibration XML -> KB4 intrinsics ([`kb4`]), stereo and body extrinsics.
//! * [`overlay`]: learned calibration corrections on top of the factory file (`pimax-selfcal` writes them;
//!   opt-in, never applied implicitly); [`json`]: the small JSON reader/writer it uses.
//! * [`clock`]: device time -> `CLOCK_MONOTONIC` (IMU reference or minimum-delay fallback).
//! * [`rectify`]: stereo rectification and the epipolar check that validates the calibration conventions.
//! * [`slam`]: 6DoF head tracking support (tracker calibration export, IMU noise, pose supervisor); C ABI in
//!   [`capi_slam`].
//! * [`ae`]: auto-exposure controller for the tracking cameras (statistics -> HID `0x51` payload; sending is the
//!   driver's job, off by default).
//! * [`headpose`]: 6DoF head pose output (IMU-rate prediction from the tracker, blended corrections, re-snap).
//! * [`capi`]: the C ABI used by the Monado driver (`include/pimax_cam.h`).
//!
//! Nothing here sends commands to the headset: capture is a plain UVC stream; [`ae`] only builds payloads.
#![warn(missing_docs)]

pub mod ae;
pub mod broker;
pub mod calib;
pub mod camera;
pub mod capi;
pub mod capi_ae;
pub mod capi_slam;
pub mod check;
pub mod clock;
pub mod frame;
pub mod geom;
pub mod headpose;
pub mod imulog;
pub mod json;
pub mod kb4;
pub mod overlay;
pub mod png;
pub mod record;
pub mod rectify;
pub mod resilient;
pub mod slam;
pub mod source;
pub mod stats;
pub mod v4l2;
pub mod xml;
