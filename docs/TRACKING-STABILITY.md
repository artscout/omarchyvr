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
- **Room light law v2** (`rust/pimax-cam/src/ae.rs`, Phase 1a of the tracking services plan): the auto exposure meters
  the masked mean of the unclipped cells (lens corners and cells more than half clipped left out); this one metric
  (`ae::scene_metric`) is also what the room setup measures (`omarchyvr-roommap` `light.rs`, "Too dark" below 30). The
  "too bright" rule needs more than 20 % clipped (released below 12 %), deadband 0.15, exposure floor 1140 us with a
  low stage down to 300 us only after 3 s of more than 30 % clipped (back after 8 s), at most 5 changes/s per pair,
  LED pair constant 40/62. `ae::is_good_frame` / `StepWatch` is the shared "good frame" (masked mean 30-190, clipped
  at most 20 %, no step landed, 1 s quiet after a big step). Kill switch: `PIMAX_CAM_AE_LAW=old` restores the previous law.

Not done: the factory accelerometer bias and the ka/kg scale terms (units unverified). To verify them: a still
recording with the headset in six orientations (+-x, +-y, +-z up) and check that |accel| is 9.81 after subtracting aBias
and applying (1 - k) per axis, with both sign conventions, then A/B the still drift on the walk replay.

- **Controller mode flips (Phase 1d, 2026-10-17)** (`rust/pimax-hands/src/fuse.rs`): the optical window is 220 ms (was
  80 ms; 25 Hz fixes plus latency left the fuser in the arm-model hold between fixes), `gap_s` 220 ms so the filter
  survives short misses (its velocity fades after 100 ms), a fix after a real hold must be confirmed by a second
  one within 150 ms and 10 cm (`confirm`, one frame of delay on re-entry only), and the fix weight falls
  continuously with the residual (1 up to 1.5 px, squared above, floor 0.3) with a lower gain for 4 LEDs or fewer.
  Replay of the 2026-10-17 16:24 logs: flips 77 and 79 per min -> 8 and 12 (what remains are real occlusions
  longer than 220 ms), still step p95 1.0 mm (unchanged), fast moves in continuous tracking differ by under 7 mm
  (under 6 ms) from the old fuser. Kill switch: `PIMAX_CTRL_TRACK_HYST=0` (`fusereplay ... --legacy` in replays).

## App switches (Phase 1e)

The Phase 1 changes are switches on the app's **Tracking** page, all on by default; each shows its status in plain
words, read from the running Monado's environment ("On in this session", "Off from the next start (this session:
on)", or a warning when the installed Monado build lacks it). The app passes the flag of a switch that is off at every
Work and Play start, so `start-argv.txt` keeps it and restarts repeat it; the launcher turns the flag into Monado's
environment unless that variable is already set. The switches are also saved in the active profile's setup.toml
(`new_exposure`, `room_glide`, `ctrl_steady`, `ctrl_angle`; `omarchyvr_config::tracking`), and a start without the
flag (the CLI, omarchyvrd) uses the saved value:

| Switch | Off = launcher flag | Monado environment | Marker (status) |
|---|---|---|---|
| New camera exposure (1a) | `--old-exposure` | `PIMAX_CAM_AE_LAW=old` | `PIMAX_CAM_AE_LAW` |
| Smooth room corrections (1b) | `--no-room-glide` | `PIMAX_ANCHOR_GLIDE=0` | `PIMAX_ANCHOR_GLIDE` (required) |
| Steadier controllers (1d) | `--no-ctrl-steady` | `PIMAX_CTRL_TRACK_HYST=0` | `PIMAX_CTRL_TRACK_HYST` |
| Controller angle slider (1c) | `--ctrl-angle DEG` (always passed) | `PIMAX_CTRL_ANGLE=DEG` | `PIMAX_CTRL_ANGLE` (required) |

The app's preflight (`omarchyvr-gui-core` `builds.rs`) and `installer/lib.sh` refuse a Monado build without
`PIMAX_CTRL_ANGLE` or `PIMAX_ANCHOR_GLIDE`. The angle is described in docs/CONTROLLERS-INHAND.md.
