//! Offline tools for 6DoF head tracking of the Pimax Dream Air (design: `analysis/research/6dof-head-tracking.md`).
//!
//! * [`export`]: Basalt calibration + VIO config + Monado `SLAM_CONFIG` toml from the factory XML.
//! * [`euroc`]: EuRoC datasets recorded by the Monado driver (`PIMAX_SLAM_RECORD`), consistency check.
//! * [`imu`]: IMU noise densities from a still segment (the factory file has none).
//! * [`timing`]: camera timing from `.pxcam` recordings.
//! * [`traj`]: trajectory comparison (ATE/RPE after rigid alignment, loop-closure drift).
//! * [`stats`]: reference-free trajectory statistics (still noise, high-frequency noise, drift per metre).
//! * [`sim`]: the driver's head pose output evaluated on a real head trajectory (synthetic IMU + tracker).
//! * [`winbench`]: our head pose output vs Pimax's own tracker on a Windows capture take (pcap + pose log).
//! * [`stillgate`]: the still gate (headset lying still, tracker dead-reckoning) replayed on a real recording.
//! * [`takehead`]: a Record-tracking take without the head IMU: the live tracker's log and the cameras at speed.
//!
//! The calibration math itself (T_imu_cam, Basalt JSON, the pose supervisor) lives in `pimax_cam::slam` so the
//! Monado driver can use it through the C ABI.
#![warn(missing_docs)]

pub mod euroc;
pub mod export;
pub mod imu;
pub mod sim;
pub mod stats;
pub mod stillgate;
pub mod takehead;
pub mod timing;
pub mod traj;
pub mod winbench;
