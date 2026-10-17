# Tracking stability (2026-10-17)

Four small, tested stability rules, learned from how Pimax Play behaves on Windows (our own code and values; only the
noise and threshold numbers come from Pimax's data). All are on by default and can be turned off for an A/B with an
environment variable.

- **Exact pose freeze** (`rust/pimax-cam/src/headpose.rs`, `PIMAX_SLAM_FREEZE=0` = off). While the headset lies on a
  desk (the existing still state, which keeps `PIMAX_SLAM_STILL` and its hold/re-zero) and the bias-corrected gyro
  stays under 0.006 rad/s, the head pose (position AND orientation) is one fixed value with zero velocities. It ends
  over 0.02 rad/s or when the still state ends, without a jump (the correction carries the difference). It re-latches
  to the live pose when that differs by more than 0.04 deg or 1 mm, so the slow gravity tilt and a still re-zero still
  arrive.
- **Two IMU noise regimes** (`pimax_cam::slam::{NoiseRegime, NoiseSelector, ImuNoise::for_regime}`): still
  gyro 0.001974 / accel 0.081686, moving 0.0225 / 1.296 (ratios applied to any measured still model).
  `HeadTracker::noise_regime()` picks the regime from the gyro rate (moving above 0.05 rad/s, still under 0.02).
  Basalt reads its noise once at start, so this is the selector and the model; nothing switches Basalt live yet.
- **Controller loss state machine** (`rust/pimax-ctrltrack/src/tracker.rs`): a slot whose prior fails 30 LED frames in
  a row goes back to init; a cold start farther than 1.8 m from the head needs 4 or more LEDs; the last rotation is
  kept (`Tracker::carried_rotation`) for the gyro to carry while the controller is out of view.
- **Windows data on a plugged-in drive** (`rust/omarchyvr-config/src/import.rs`): besides `/mnt/shared`, every
  `/run/media/$USER/*` mount is searched for an `omarchyvr-windows` folder or `ProgramData-Pimax`.

Not done: the factory accelerometer bias and the ka/kg scale terms (units unverified). To verify them: a still
recording with the headset in six orientations (+-x, +-y, +-z up) and check that |accel| is 9.81 after subtracting aBias
and applying (1 - k) per axis, with both sign conventions, then A/B the still drift on the walk replay.
