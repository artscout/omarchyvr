//! Camera passthrough for the Pimax Dream Air (design: `analysis/passthrough-design.md`).
//!
//! The four tracking cameras are grey 640x480 KB4 fisheyes. For each eye we precompute a map (LUT) from every
//! pixel of the eye's pinhole image (the OpenXR view FOV, before lens distortion) to camera pixels:
//!
//! 1. eye pixel -> ray in the eye frame ([`view::EyeView::ray`]);
//! 2. ray -> world point at a fixed distance `D` from the eye (or a pure direction for `D = inf`);
//! 3. head frame -> camera frame with the factory extrinsics ([`rig::Rig`]);
//! 4. camera frame -> fisheye pixel with KB4 ([`pimax_cam::kb4::Kb4::project`]);
//! 5. camera choice per pixel: a preference order (the camera on the eye's side first) with a feathered seam.
//!
//! The map depends only on head-fixed geometry, so it is built once. Per camera frame the work is a bilinear
//! lookup ([`remap`]). Head rotation between camera exposure and display is not handled here: the live client
//! ([`xr`]) submits each image with the eye pose *at camera time* in a world-fixed space, and the Monado
//! compositor's timewarp rotates it to the display-time pose (exact for `D = inf`, rotation-only otherwise).
#![warn(missing_docs)]

pub mod check;
pub mod floorgrid;
pub mod control;
pub mod frames;
pub mod lut;
pub mod pngdec;
pub mod remap;
pub mod rig;
pub mod timing;
pub mod view;
pub mod xr;
