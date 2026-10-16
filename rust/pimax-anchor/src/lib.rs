//! Room anchor for the Pimax Dream Air.
//!
//! A printed AprilTag (tag36h11) lies on the desk. The headset cameras see it, `pimax-anchor` measures its pose in the
//! head frame (KB4 fisheye model, joint refinement over all cameras that see it), combines it with the head pose Monado
//! reports at the same instant and so knows where the tag is in the current (SLAM) world. The offset between that and
//! the world pose saved at calibration time goes to a small file that the Monado `desk` branch applies to the head
//! pose (and with it to controllers, gaze and everything composed on it), so the VR windows come back to the same real
//! spot every session.
#![allow(
    clippy::needless_range_loop,
    clippy::too_many_arguments,
    clippy::type_complexity
)]
pub mod config;
pub mod detect;
pub mod rig;
pub mod tag;
pub mod world;
