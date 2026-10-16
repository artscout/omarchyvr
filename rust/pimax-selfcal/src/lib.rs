//! Self-calibration of the Pimax Dream Air tracking-camera rig from recorded (later live) frames.
//!
//! The factory calibration (`pimax_cam::calib`) is good to about one pixel; the residual is a small rotation
//! between cameras (the morning check showed ~1 px of vertical disparity in the trackingA/B rectified pair, the
//! passthrough check a constant ~0.6 deg vertical offset between the cameras' eye views). This crate estimates
//! per-camera rotation corrections and writes them as a learned-calibration overlay next to the factory file
//! (`pimax_cam::overlay`), never into it.
//!
//! * [`scene`]: input frames (tile PNGs or `.pxcam` recordings, tracking frames only).
//! * [`rect`]: stereo rectification of a camera pair (float images + validity mask).
//! * [`matcher`]: corner matches on a rectified pair with Lucas-Kanade sub-pixel refinement.
//! * [`ba`]: rotation-only bundle adjustment over all pairs (inverse depths eliminated by a Schur complement).
//! * [`pipeline`]: rectify + match every pair of every scene, BA rounds, before/after measurement, hold-out
//!   validation, the sanity gate and the overlay.
//! * [`diag`]: which error explains a pair's vertical disparities (rotation, baseline direction, neither).
//! * [`eyecheck`]: the same error measured in the passthrough's eye views (camera A only versus camera B only).
//! * [`circle`]: lens image circle versus the calibration's principal point (an intrinsics check without matching).
//! * [`synth`]: a synthetic textured room rendered through any calibration (tests, `selftest`).
//! * [`viz`]: pictures (matches and their errors on the rectified pairs, before/after).
//! * [`so3`]: rotation helpers.
//!
//! Nothing here opens the headset: input is recorded data.
#![warn(missing_docs)]
// Index loops read better than iterator chains in the small dense linear algebra (3x3 blocks, normal equations).
#![allow(clippy::needless_range_loop)]

pub mod ba;
pub mod circle;
pub mod diag;
pub mod eyecheck;
pub mod matcher;
pub mod pipeline;
pub mod rect;
pub mod scene;
pub mod so3;
pub mod synth;
pub mod viz;
