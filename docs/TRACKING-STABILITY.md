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
(`new_exposure`, `room_glide`, `ctrl_steady`, `drift_hold`, `ctrl_angle`; `omarchyvr_config::tracking`), and a start without the
flag (the CLI, omarchyvrd) uses the saved value:

| Switch | Off = launcher flag | Monado environment | Marker (status) |
|---|---|---|---|
| New camera exposure (1a) | `--old-exposure` | `PIMAX_CAM_AE_LAW=old` | `PIMAX_CAM_AE_LAW` |
| Smooth room corrections (1b) | `--no-room-glide` | `PIMAX_ANCHOR_GLIDE=0` | `PIMAX_ANCHOR_GLIDE` (required) |
| Steadier controllers (1d) | `--no-ctrl-steady` | `PIMAX_CTRL_TRACK_HYST=0` | `PIMAX_CTRL_TRACK_HYST` |
| Controller angle slider (1c) | `--ctrl-angle DEG` (always passed) | `PIMAX_CTRL_ANGLE=DEG` | `PIMAX_CTRL_ANGLE` (required) |
| Hold the head when it does not move (2026-10-18) | `--no-drift-hold` | `PIMAX_SLAM_DRIFT_HOLD=0` | `PIMAX_SLAM_DRIFT_HOLD` |

The app's preflight (`omarchyvr-gui-core` `builds.rs`) and `installer/lib.sh` refuse a Monado build without
`PIMAX_CTRL_ANGLE` or `PIMAX_ANCHOR_GLIDE`. The angle is described in docs/CONTROLLERS-INHAND.md.

## Drift hold and the gyro bias at rest (2026-10-18)

Seated session 2026-10-18 11:58-12:06: Basalt's position slid 4.1 m right and 1.6 m up in 8 minutes while Monado
said "0 lost"; the output followed it (corrections 0-6 mm). The yaw drifted only about 1.3 deg/min. Kept data:
`.claude/lab/drift-2026-10-18/` (private). Lesson 77.

- **Drift hold** (`rust/pimax-cam/src/drift.rs`, used by `headpose.rs`; switch "Hold the head when it does not move",
  `PIMAX_SLAM_DRIFT_HOLD=0` = off). The governor looks at the neck (`neck_in_head`, 10 cm below and 8 cm behind the
  head origin): turning moves the head on an arc around it, which the rotation explains; the neck itself moves only
  when the body moves, and that shows in the accelerometer. Per tracker pose the neck's speed over the last 0.25 s is
  compared with what the accelerometer allows:
  - not turning: the spread (root of the summed per-axis variances over 1 s) of the neck's world linear acceleration
    above 0.10 m/s² allows 1 m/s per m/s² (a worn head at rest shows 0.05-0.13, a lean 0.1-0.5, steps 0.5-2);
  - turning (gyro over 15 deg/s within 0.5 s): the world value is not trusted (the neck is known to a few cm), only the
    spread of |neck specific force| over 1 s above 0.30 m/s² counts (rotation does not change a magnitude; fast turns
    alone reach 0.15-0.2, steps 0.5-2).
  What exceeds the allowed speed is moved into the room alignment (`Supervisor::shift_world`), completely up to
  5 cm/s (10 cm/s while turning) and fading to nothing at 15 cm/s (25 cm/s): Basalt's slide is slow, and faster
  motion is left to the tracker. Rotation is never touched. An accelerometer variance, not an integral: integrating
  it (the first try) made 2-3 cm/s of "evidence" out of the bias and of gravity leaking through the attitude.
  Every 10 s with at least 1 cm held, Monado's log says `6DoF drift: head tracking slid N cm without movement in 10 s;
  held (session total T cm)`; the app shows "Head tracking slid N cm without movement; held (T cm this session)" on
  the Tracking page and in the switch's status.
- **Replay of the session** (`PIMAX_REPLAY_HEAD_IMU=<head-imu.csv> cargo test --release --lib -p pimax-cam
  replay_drift_hold -- --ignored --nocapture`): the output moved 4.30 m -> 0.34 m over the whole take
  (11:58:08-12:06:15), 51 cm -> 4.0 cm in the quiet 20 s (12:05:31-12:05:51, 2.6 -> 0.2 cm/s) and 1.57 m -> 0.14 m
  during the fast turns (12:00:28-12:01:38). Where the accelerometer shows motion (0.5 s steps with world spread
  > 0.2 m/s² while not turning or |a| spread > 0.4, tracker moved > 3 cm) the output kept 86 % of the tracker's path
  (98 % before; part of that path was slide too).
- **Synthetic tests** (`headpose.rs` `drift_hold_*`): a quiet seated head with a tracker sliding 2.6 cm/s for 20 s:
  0.1 cm (52.6 cm without the hold); fast turns (+-60 deg at 0.5 Hz) with 2.1 cm/s of slide for 30 s: 0.5 cm
  (62.4 cm), the head's arc around the neck still 5.4 cm of the true 5.5 cm. Passing through: a 20 cm lean in 1 s
  (100 %), a 25 cm sideways lean in 1.5 s (100 %), standing up 45 cm in 1.2 s (100 %), a 2.1 m walk in 3 s with head
  bob (100 %). The test walk of the older output tests now bobs 1.5 cm at 1.8 Hz like a real walk; a bob-free glide
  is indistinguishable from slide. **Limit:** a very slow lean (10 cm over 2 s, 0.12 m/s² peak) is held (0.6 of
  10 cm pass): its acceleration is within a worn head's own.
- **Gyro bias only from a real rest** (Monado `src/xrt/drivers/pimax/pimax_bias.h`, used by `pimax_hmd.c`). The old
  rule (every sample |gx|+|gy|+|gz| < 11 deg/s and half means within 0.3 deg/s, refined once per session) cached a worn
  window at 11:59:47 (head tilted 23 deg, gyro sd 0.5 deg/s): 0.185 0.892 0.418 deg/s against the desk rest 0.118 0.775
  0.370. Now a 2 s window counts only with every axis' gyro sd under 0.15 deg/s AND accelerometer sd under 0.03 m/s², and
  a mean within 0.3 deg/s of the bias in use (the cache at start) unless the window after it agrees within 0.05 deg/s
  (the old value was wrong). Failing windows are never cached; every real rest of the session refines the bias (the
  cache is rewritten when it moved over 0.01 deg/s or after 5 min). Replay of the session through the exact C code
  (`pimax_bias_test.c`): 0 worn windows accepted (the old rule took 48, the first the 11:59:47 one with the same mean
  as the cache), 16 desk windows accepted at +0.115..0.127 +0.773..0.782 +0.368..0.371 deg/s.
- **Head IMU logs are kept** (`rust/pimax-cam/src/imulog.rs` `rotate`): a new session renames the previous
  `head-imu.csv` to `head-imu.<YYYYmmdd-HHMMSS>.csv` (its last write time), links `head-imu.csv.prev` to it and keeps
  the newest 3 (about 45 MB each).
