//! Controller-driven hand tracking for the Pimax Dream Air.
//!
//! The controllers report trigger, grip, touch bits and buttons but no finger positions. This crate turns those
//! into plausible finger curls and tells the Monado driver where the wrist is relative to the controller's grip pose,
//! so `u_hand_sim_simulate_generic` can produce an `XR_EXT_hand_tracking` joint set with data source `CONTROLLER`.
//!
//! * [`filter`]: One Euro filter and a critically damped smoother
//! * [`report`]: decoded controller report -> input
//! * [`mapping`]: input -> curl mapping, thumb state machine, curl -> joint curls (Monado's layout)
//! * [`pose`]: grip pose -> wrist offset (inverse of Monado's `ht_ctrl_emu` grip rule)
//! * [`rest`]: where the untracked controllers are assumed to be (aim / grip pose from the head orientation)
//! * [`inhand`]: is a controller in the hand or lying on the desk (PIMAX_CTRL_INHAND), C ABI `include/pimax_inhand.h`
//! * [`touch`]: grip of a tracked controller from its aim (Oculus Touch / Quest 2 geometry, same as SteamVR)
//! * [`demo`]: scripted input for end-to-end tests without a human
//! * [`ffi`]: the C ABI (`include/pimax_hands.h`)
//!
//! Design notes and test steps: `analysis/overnight/hands.md`.
#![deny(unsafe_code)]

pub mod demo;
#[allow(unsafe_code)]
pub mod ffi;
pub mod filter;
pub mod fuse;
#[allow(unsafe_code)]
pub mod fuse_ffi;
pub mod inhand;
#[allow(unsafe_code)]
pub mod inhand_ffi;
pub mod mapping;
pub mod pose;
pub mod report;
pub mod rest;
pub mod touch;

pub use mapping::{flags, Curls, FingerJoints, HandJoints, HandState, Input, Params, ThumbState};
pub use pose::{grip_from_joints, grip_to_wrist, Hand, Pose};
pub use report::{input_from_report, Report};
pub use rest::{controller_poses, AimMode, Rest};
