# 6DoF drift on the 2026-10-14 walk: root cause and fix

Recording `~/.local/share/omarchyvr/slam-recordings/20261014-152215-live` (94 s, AE off). Live: 2.2 m drift in the
first 5 s "standing still", 6.5 m loop error (23 %). Offline only (Basalt via `monado-cli slambatch`,
deterministic, max speed gives the same tracks as real time).

## Root cause

1. **The still phase was not a standing user.** The IMU is perfectly still for 13.8 s (gyro std 0.06 deg/s,
   accel std 0.01 m/s², desk-level) and both cameras see sofa fabric a few cm away, out of focus. The earlier
   recording `151524` starts the same way (headset on the sofa, fabric close). The headset was lying down, then
   picked up at 13.8 s and put back on the cushion at ~90 s.
2. **No landmarks while still.** Basalt only creates landmarks by triangulation: stereo (cam0/cam1 overlap,
   depth must be > 0.33 m, `inv_dist < 3` hard-coded) or motion (> 5 cm). Fabric in the overlap gives
   nothing (SIFT: 1 of 31 stereo matches passes the epipolar gate at the start; `vio_debug`: `num_lms 0` for
   the first 55 frames). So Basalt integrates the IMU alone.
3. **Gyro bias not seeded.** Basalt starts its gyro bias at 0; the real bias is (-0.23, +0.73, -0.41) deg/s
   (body), stable across sessions (driver cache `~/.cache/omarchyvr/gyro-bias`, 151524, imu-orientation.md). In the
   landmark-free first ~1.5 s the attitude turns by bias x time (Basalt's orientation at 1 s = exactly the raw
   bias: -0.20, +0.75, -0.42 deg). Pitch/roll stay wrong (0.38 deg about body z) once landmarks lock that pose,
   and gravity leaks into position: 9.81 x sin(0.39 deg) = 0.067 m/s², fitted 0.071 m/s² -> 3.6 m at 10 s,
   6.7 m at pickup. The walk itself was fine: end vs pickup position = 0.38 m over 22.5 m (1.7 %). The whole
   "loop error" is the still phase.
4. **IMU noise over-confident.** Our datasheet-based densities (gyro 4e-4, accel 4e-3) let Basalt trust the
   IMU over vision; Pimax's own stationary densities (gyro 0.001974, accel 0.081686, from the older Windows
   calibration, windows-6dof-params.md) do much better.

Checked and **not** the cause:

| Suspect | Evidence |
|---|---|
| IMU units/axes | still \|a\| = 9.817 m/s² (+y up, specific force); gyro rad/s: camera rotation from images (essential matrix, 467 frame pairs) vs integrated gyro, median ratio 0.99 |
| IMU-camera rotation (`ombc`, direction) | hand-eye fit 1.4 deg (cam0) / 1.5 deg (cam1) from the calibration; inverse convention gives 2.6 deg epipolar error vs 0.29 deg |
| Camera-IMU time offset | hand-eye best offset -2..+2 ms; camera shift of +1.638 ms: loop 24.0 % (walk 1.8 %), -1.638 ms: 22.7 % (walk 1.4 %) vs 24.0 % / 1.7 % at 0. Irrelevant while still, < 0.4 % on the walk |
| Extrinsics from the Windows ProgramData XML | byte-identical to `analysis/headset-calibration.xml`. The older ProgramFiles XML (other rig, different `tbc`/`ombc`) is wrong for this headset: loop 35 %, walk 45 % |
| Stereo baseline / intrinsics | baseline 111.6 mm; cam1 rotation off by ~0.3-0.6 deg (selfcal agrees), refitting it changed the walk 1.3 -> 1.3 %; epipolar gate 0.005 -> 0.01: no change while still |
| Frame time (mid-exposure) | offset sweep above is centred on 0 |
| The old 0.38 mm jitter | that was `omarchyvr-slam simulate` (head pose output on the Windows trajectory with a synthetic tracker), not a Basalt run |

## Numbers (152215, offline)

| Run | drift at 5 s still | drift before pickup | loop end-start |
|---|---|---|---|
| before (export as it was) | 92 cm | 6.66 m | 6.99 m / 29.2 m = 24.0 % |
| + Pimax noise only | 25 cm | 0.92 m | 4.3 % |
| + Pimax noise, bias walks x5 | 62 cm | 3.36 m | 5.2 % |
| + gyro bias seed only | 10.3 cm | 1.85 m | 6.8 % |
| **seed + Pimax noise (new export, `run_slam.sh replay`)** | **7.5 cm** | **7 cm** | **0.37 m / 20.9 m = 1.8 %** |
| same + driver still hold/reset (emulated) | **0.0 cm** (output held) | 0.4 cm | 0.44 m / 21.1 m = 2.1 % |

The headset was put back on the cushion with a 1.7 deg different attitude, so a few cm-dm of the closure may be
real. The emulation re-runs Basalt from each reset time (`trimstart` + slambatch), with the driver's rules
(acquire after 12 poses, IMU-still test, hold, reset at 3 cm / at most 1 per s): resets at 2.7, 4.7, 7.6, 9.8,
12.8 s while lying on the cushion, then none during the walk.

## Changes

* `pimax-cam` `SlamSetup.gyro_bias` -> Basalt `calib_gyro_bias`; driver-written recording calibrations use
  the driver's bias cache. `omarchyvr-slam export --gyro-bias X,Y,Z|none --bias-from DATASET` (default: the
  driver's cache, raw dps converted to body rad/s). `run_slam.sh replay` seeds from the recording's first still
  second.
* `ImuNoise::default()` = Pimax's stationary densities (gyro 0.001974, accel 0.081686); bias walks unchanged.
* Supervisor `StillParams` (driver, Rust): when the IMU is desk-still (0.5 s window, std < 0.004 rad/s and
  0.03 m/s²; a worn headset never gets that quiet) the head position is held (tracker motion goes into the
  room alignment, VIO velocity zeroed for the predictor) and a tracker that drifts 3 cm meanwhile is reset
  (fresh start with zero velocity and the right attitude), at most once per second. `PIMAX_SLAM_STILL=0`
  turns it off, `=N` sets the reset distance in cm. Monado logs the reset reason.
* `run_slam.sh` picks the newest omarchyvr-slam build that has `stats` (the old `$CARGO_TARGET_DIR` one did not).

## Rest -> wear (Windows drifts too)

The user reports Pimax's Windows tracker drifts badly, mostly vertically, and after every rest/put-back. That
fits the same mechanism: a resting headset sees nothing triangulable (desk, cushion, or cameras idle) and a
VIO dead-reckons; any vertical accel error (here \|a\| 9.794 vs 9.81 in `151524`, 0.016 m/s² = 20 cm in 5 s)
goes straight into height. Pimax's active calibration has zero IMU noise and a `wBias` that does not match the
real bias (imu-orientation.md §3). Not checked yet: whether our driver idles cameras or IMU when the proximity
sensor says off; if it does, the tracker must be reset on wear (zero velocity, attitude from the first still
accel), which the still rule above already does whenever the headset lies still before being put on.

## Open

* Raw Basalt still drifts 7.5 cm in 5 s on this degenerate start; only the driver hold gets the output under
  5 cm. A Basalt-side static init (attitude from the mean of the first still samples, zero-velocity factor)
  would fix it at the source.
* Live test (needs the user, AE off is fine):
  `sudo setcap cap_sys_nice+ep ~/.local/lib/omarchyvr/monado/src/xrt/targets/service/monado-service` then
  `PIMAX_CAM_AE=0 ~/omarchyvr/analysis/tools/run_slam_view.sh 90`: leave the headset on the cushion 10 s
  (log must show `resetting the tracker (tracker drifting while still ...)` and `p=` unchanged), walk the loop,
  put it back on the same spot.

## Live test 2026-10-14 16:24 (`20261014-162452-live`)

* Bias was seeded live (driver cache, within 0.02 deg/s of the recording's still mean). The still rule fired only
  while resting (5 resets before pickup at 8.8 s, 21 on the cushion at the end), never during the walk.
* `poses.txt` starts about 11 s after the recording (posecli connects late), i.e. after pickup: its "first 10 s"
  are the walk and its end-start is not a loop. The headset also came to rest with a different attitude.
* Real defect: at the end rest the accel std was 0.022-0.05 m/s² (cushion/fan), flickering around the single
  0.03 limit; each flicker re-anchored the hold on the drifted tracker, so the held output crept 1-4 cm per
  reset. Fix: hysteresis (enter gyro < 0.006 rad/s and accel < 0.06 m/s², leave at 0.012 / 0.15; worn walks
  never go below 0.027 rad/s). Emulated with the fix: output constant over the 50 s rest, pre-pickup -> end
  0.23 m / 19.6 m (1.2 %); raw Basalt without the driver rule: 39.7 % (dead-reckoning the 50 s rest).
* Not reproduced offline: two "position jump" losses at pickup with a 0.58 m blended correction (live Basalt in
  real time just after a reset); the offline tracker stays under 1.5 m/s there.
* The follow-up live run `20261014-164606-live` (60 s, AE on): 600/605 lines 6dof, 0 lost, user: "not jumping".

## Still gate: re-zero instead of a dropout (2026-10-15)

Live Play session 17:15 (`/tmp/omarchyvr-session/monado.log`, ~3 min): 43 x `6DoF: lost (tracker drifting while
still)`, then `tracking (re-aligned)`, rotation corrections up to 2.8 deg. All 43 fired while the headset lay
still, none while it was worn: 8 in the first 13 s (headset on the desk, gyro calibration window, Basalt's first
`T_w_i` has the start attitude) and 35 after the game closed (`Client 5 disconnected`, then `T_w_i` with up =
+y in the IMU frame, i.e. headset lying upside down) until Stop. The previous session (`monado.log.prev`) shows
the same: 3 at the start, none during play.

* **Not false positives**: the IMU test (0.5 s, std < 0.006 rad/s and 0.06 m/s², leave at 0.012 / 0.15) never
  passed during worn use in any recording; the fresh Basalt really dead-reckons 3 cm in 1.4-2 s with nothing in
  the stereo overlap (desk / lenses down), every time. The reset is right; what was wrong is how it was reported
  and output: every firing was a 6DoF loss (3DoF for 12 tracker poses ~0.5 s + the restart, counted in "lost"),
  and the 3DoF fallback used a yaw-only fix, so the view tilted by the difference between the 3DoF filter's and
  Basalt's attitude (1-3 deg) at both ends of each gap. A pickup inside such a gap moved the head while the
  position was held (room shifted by that motion).
* **Fix** (`pimax-cam` `Supervisor::rezero`, `StillParams.rezero`, default on): the tracker is still reset at
  3 cm (at most once per second), but as a **re-zero**: no loss, no LOST event, the output keeps the held
  position and the tracker's full attitude (the 3DoF fix carries tilt too), and the fresh tracker world takes
  over after 3 consistent poses (`rezero_poses`, was 12) aligned to that pose silently. Monado logs
  `6DoF: tracker re-zeroed while still (position held, no dropout; reset N)` at INFO; the status line says
  `6DoF tracking (re-zeroing while still, position held)`; the app counts it as a reset, not a loss.
  `PIMAX_SLAM_STILL_REZERO=0` restores the old drop for A/B.
* **Offline** (`omarchyvr-slam stillgate DIR`: the driver's HeadTracker on the recording's IMU + its Basalt run,
  tracker resets emulated as a fresh zero-velocity world, 90 Hz queries), old -> new:

| Recording | still | gate fired | 6DoF lost | time without 6DoF |
|---|---|---|---|---|
| 162452 (rest 9 s, walk, rest 50 s) | 59 s | 49 -> 50 | **49 -> 0** | 26.9 -> 9.5 s (output held, headset lying) |
| 152215 | 17 s | 5 -> 5 | **5 -> 0** | 2.4 -> 1.0 s |
| 151524 | 17 s | 4 -> 4 | **4 -> 0** | 2.3 -> 0.8 s |
| 163703 | 12 s | 4 -> 4 | **4 -> 0** | 2.3 -> 0.8 s |
| 164420 | 16 s | 3 -> 3 | **3 -> 0** | 1.7 -> 0.6 s |
| 164606 (worn walk) | 4 s | 0 -> 0 | 0 -> 0 | 0 |

  Real drift is still caught (same firings); while still the output moves < 3 mm / 1.3 deg over a whole
  minute (emulated tracker attitude drift), no step above 0.4 mm / 0.07 deg. Unit tests:
  `still_headset_holds_and_rezeroes_a_drifting_tracker_without_a_loss`, `still_rezero_off_is_the_old_dropout`
  (slam.rs), `lying_still_rezero_holds_the_output_without_dropouts` (stillgate.rs, tilt swing 1.45 -> 0.09 deg).
* **What the user feels while playing is something else**: in the worn phase of both sessions there was no gate
  firing, but per-pose corrections of 45-74 mm (another session: up to 150 mm) were blended in at the rest
  rate limit (0.1 m/s, so a 74 mm correction slides for ~0.7 s). Those come from Basalt itself (pose vs
  IMU-predicted pose); next step: log each correction above 20 mm with time and head speed and replay one.

## Corrections while worn: sliding and "the head moves a little up and down" (2026-10-15)

Every new tracker pose (25 Hz) replaces the base the output is dead-reckoned from (`headpose.rs`); the difference
between the old prediction and the new pose goes into a correction offset that decays (150 ms, rate limited).
In the worn phase of today's sessions no 6DoF loss happened, but offsets reached 45-74 mm (another session:
150 mm) and slid off at up to ~0.1 m/s with the head at rest: that is the sliding. Cause, from the replay: the
prediction starts at the newest pose with the tracker's newest velocity, so the pose-to-pose noise of Basalt's
position and its noisy newest velocity (x 45-85 ms of latency + prediction) arrive as steps; most big steps
come in head motion (0.7 m/s mean), 13 of 100 at under 0.25 m/s (the visible ones), 20 % mostly vertical.

`omarchyvr-slam corrections DIR [--central-vel]` replays a recording through the output with every correction
logged and measures: steps >= 20 mm (vertical / at < 0.25 m/s), largest pending offset, jitter while worn and
quiet (position minus its 0.5 s / 2 s average, vertical), sliding at rest (speed of the decaying offset while
the head moves < 0.25 m/s, time above 2 cm/s), and the error against the hindsight tracker trajectory.
Velocity: `--central-vel` matches the live logs best (164606 live max 95.3 mm, replay 96.6 mm); the causal
default is a pessimistic bound.

Two changes, both on by default, both switchable:

* **Fused position** (`PIMAX_SLAM_FUSE`, default `0.15,0.075,0.06,0.5,0.5`; `0` = off): each tracker pose
  only pulls the IMU-propagated position by 15 % (7.5 % vertically) and the velocity by 0.06 x difference /
  interval plus half the tracker's velocity difference; the tracker's velocity weight rises to 1 at 0.5 m/s, so
  fast motion keeps the tracker's own velocity (no lag). An alpha-beta filter driven by the accelerometer.
* **Slow decay at rest** (`PIMAX_SLAM_TAU_REST_MS`, default 400; `0` = off): pending position corrections decay
  with 400 ms at rest, going over to 150 ms at 0.3 m/s head speed: they slide slowly where they would show and
  are hidden in motion.

Sums over 6 recordings (164606, 163703, 164420, 152215, 162452, 151524), `--central-vel`:

| | before | fused only | rest decay only | **now** |
|---|---|---|---|---|
| corrections >= 20 mm | 100 | 60 | 100 | **60** |
| ... mostly vertical | 20 | 9 | 20 | **9** |
| ... at < 0.25 m/s (visible) | 13 | 5 | 13 | **5** |
| largest pending offset | 149 mm | 114 mm | 149 mm | **114 mm** |
| vertical jitter while quiet (sum of RMS) | 1.86 mm | 1.65 | 1.69 | **1.55 mm** |
| sliding > 2 cm/s at rest | 32.5 s | 24.5 s | 21.7 s | **15.0 s** |
| worst sliding RMS | 110 mm/s | 54 | 112 | **53 mm/s** |
| error vs hindsight trajectory (sum of RMS) | 35.5 mm | 38.4 | 38.5 | 42.3 mm |

With the causal velocity: >= 20 mm 294 -> 157 (visible 68 -> 19), sliding 54 -> 21 s. The cost is a little
lag (error +1.2 mm RMS per recording on average). Vertical wander over 2 s is 0.6 mm in all variants: in these
recordings the tracker itself does not bob while quiet; the vertical up-and-down was the vertical corrections
(20 -> 9) and their sliding.

Live: Monado logs every step >= 20 mm (`PIMAX_SLAM_BIG_CORR_MM`) at the 5 s stats as
`6DoF correction: N mm (vertical|horizontal: vertical V, horizontal H), pending P mm / D deg, head S m/s W deg/s,
tracker pose|supervisor, t T s` (8 per period, then a count). Next: check those lines in a seated session; if
vertical steps remain at rest, try Basalt's IMU noise / keyframe settings (needs `run_slam.sh replay`, real
time) or anchor height to the saved floor.

While the headset lies still (IMU desk-still) the predictor no longer integrates the accelerometer between poses
(its bias only added drift): the synthetic lying test's output moved 0.2 mm instead of 2.8 mm.
