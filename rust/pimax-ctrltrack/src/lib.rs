//! Offline controller constellation tracking (Pimax Dream Air controllers seen in the headset's LED frames).
#![allow(
    clippy::needless_range_loop,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::manual_is_multiple_of
)]
pub mod assign;
pub mod blob;
pub mod extrinsics;
pub mod fusereplay;
pub mod imufit;
pub mod model;
pub mod occlusion;
pub mod p3p;
pub mod shm;
pub mod solve;
