//! Analysis helpers of `xr-handtest` (no OpenXR runtime needed, so they are unit tested against golden data).
//!
//! * [`joints`]: bone angles, finger flexion and the curl estimates derived from a located joint set
//! * [`checks`]: pass/fail rules (grip pose consistency, scripted demo pattern)
//! * [`pinch`]: `xr-pinchtest` bookkeeping and verdict (hand-interaction pinch = controller trigger)
pub mod checks;
pub mod joints;
pub mod pinch;
