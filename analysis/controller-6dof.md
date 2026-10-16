# 6DoF controller tracking (LED constellation): phase 1, offline (2026-10-14)

Goal: real 6DoF poses for the two Pimax Dream Air controllers. The headset cameras see the controllers' IR LEDs in the
LED frames, and the poses get fused with the controller IMU, then fed to Monado (`forks/monado` branch `slam`).
Phase 1 was offline only. No HID or USB command was sent, and neither Monado nor the headset was started.

## Summary

1. **The IR LEDs are on in the Linux recording.** `~/ctrl-test.pxcam` has 1492 LED frames (exposure 40, gain 62 on all
   four cameras). They show two clusters of 5-8 sharp blobs per camera while the controllers are in view (image 1). So
   the missing green light or vibration (`analysis/controller-activation.md`) does not block optical tracking.
2. **The first working slice is in place: blobs, LED IDs, PnP and a joint 4-camera pose with reprojection error.** The
   tool is `rust/pimax-ctrltrack`. For the 40 s with the controllers in front (`--fit-extrinsics`):
   * At least one controller is found in **81 %** of LED frames, and both in 48 %. From 12-20 s: 100 % and 76 %.
   * Median joint reprojection RMS is **0.68 px** (p90 1.16 px). The median pose uses 11 matched LEDs over 2-4 cameras.
   * Median distance is 0.35-0.39 m, 3-15 cm below the eyes, left hand at -x and right hand at +x.
   * The shape fit tells left from right correctly in **905 of 962** two-controller poses.
   * Speed: 1492 frames take 28 s on 32 threads, 14 s of that being blob extraction.
3. **Finding: the factory extrinsics of cameras 2 and 3 are off.** The controller is a rigid object of known shape seen
   by several cameras, so it works as a calibration target. Refitting cameras 1-3 (camera 0 fixed) gives these
   rotation corrections: cam1 **0.46 deg**, cam2 **1.45 deg**, cam3 **3.2 deg**. The refit lowers the median RMS from
   1.11 to 0.68 px and raises the matched LEDs per pose from 8 to 11.
   * This agrees with the earlier self-calibration report (`analysis/selfcal/real_report.txt`): pairs with cam 2 or 3
     have 2-8 px vertical disparity.
   * Cam 3 still leaves 3.8 px RMS after a rigid refit, so it has a further non-rigid error (see section 4).
   * SLAM uses cams 0 and 1 only, so it is unaffected.
4. **Architecture decision: use Monado's in-tree constellation tracker for the live path. Rust stays the offline
   harness and calibration tool.** Reasons in section 3.
5. **Remaining estimate: about 2 weeks** (section 5). The user's next step is one combined camera + controller-HID
   recording (section 6).

| 1: LED frame, blobs boxed (rec 701) |
|---|
| ![](controller-6dof/led-frame-rec00701-blobs.jpg) |

| 2: solved poses projected back (rec 721) | 3: solved poses projected back (rec 1801) |
|---|---|
| ![](controller-6dof/overlay-rec00721.jpg) | ![](controller-6dof/overlay-rec01801.jpg) |

How to read images 2 and 3:

* The four camera tiles are cam 0-3, left to right.
* Red boxes are detected blobs.
* Each solved pose is projected back as model LEDs: green = matched to a blob, blue = facing the camera but unmatched.
* The left controller's LEDs are drawn as dots, the right controller's as rings.
* Cams 0-2 land on the blobs. Cam 3 (right tile) is still off by several pixels.

## 1. Feasibility (questions answered)

| item | answer | source |
|---|---|---|
| LED model | 15 LEDs per controller: position + unit normal in metres, a ring about 9 x 7 x 6 cm. Also `ImuToDevice` (4x4) and `Hand`. Left and right are exact mirror images in x. Each model is also *nearly* mirror-symmetric in itself (differences about 5 mm), which is why the left/right label from shape alone is right only 94 % of the time. | `pimax-data/ProgramData-Pimax/slam/H1/<serial>/controller_params.json` (`H30100...` = left, `H30101...` = right). The per-serial files equal Pimax's default `test_0`/`test_1` models (rounding only), so there is no per-unit LED calibration. |
| LED exposure mode | Yes, built in. Frames alternate between tracking frames (`0x86`) and LED frames (`0x06`), 25 Hz each. The LED frame exposure is 40 / gain 62. No command is needed. Background (windows, lamps) still shows in LED frames. The blob filter (max side 40 px, peak >= 120) and RANSAC handle it. | `pimax-cam` `frame.rs`, `analysis/overnight/cam.md` |
| Which cameras see the controllers | All four, depending on the pose. For hands held in front at chest height, cams **0 and 2** (left side) and **1** see most LEDs. Cam 3 sees them at its lower edge. Note: per the factory calibration, cams 0 and 1 ("tracking") look **25 deg down** and cams 2 and 3 ("Pi-controller") look **21 deg up**. The hands at the waist (40-48 s of the recording) are lost by all cameras. | `pimax-cam calib` body-frame report, tracker statistics |
| Controller IMU | EP 0x82, 64 B report, about 250 reports/s per controller with one or two samples each. The samples are big-endian int16: accel about **1024 LSB/g** (verified at rest); gyro scale **unknown** (Monado guesses 16.4 LSB/(deg/s), `PIMAX_CTRL_GYRO_LSB_DPS`). Axes vs the LED model: `ImuToDevice` in the JSON. | `analysis/controllers-settings.md` R2.4, Monado `pimax_ctrl.c` |
| Time sync | **Free:** the controller report `timestamp_us` (bytes 8..11) is the headset MCU microsecond clock. The camera metadata u64 microsecond timestamp is the same MCU clock (its low 32 bits match the headset IMU u32). Remaining unknowns: the exposure mid-point offset (a fixed number, fit it from data) and the radio latency between the controller IMU sample and the stamp. Note: Monado's `pimax_ctrl.c` stamps samples with the host `now`; it must use the device stamp for fusion. | `analysis/overnight/cam.md`, `controllers-settings.md` |
| LED blink / IDs | Not needed: brute-force correspondence identifies the LEDs. Byte 45 (00/ff toggling) may be the LED sync phase. If so, it could also tell the two controllers apart (unverified). | |

## 2. What was built (offline slice)

Crate `rust/pimax-ctrltrack` (commits 0b5c997 and 38ccea2; 7 unit tests; clippy clean). It depends only on `pimax-cam`
(recording reader, KB4 model, factory calibration).

* `blob.rs`: blob detection on the 640x480 LED tiles.
  * Threshold, then 8-connected union-find labelling.
  * Intensity-weighted sub-pixel centroid.
  * Filters on peak brightness, area and bounding-box size.
* `model.rs`: loader for `controller_params.json`. It also builds the P3P hypothesis triples: ordered LED triples
  whose LEDs are within 4.5 cm of each other with normals within 110 deg (102 triples per model).
* `p3p.rs`: Grunert P3P (quartic solved by Durand-Kerner, then Newton), followed by an exact triad alignment.
* `solve.rs`:
  * **Acquisition** without a prior:
    * Each blob is paired with two of its 5 nearest neighbours. These triples are tried against all LED triples.
    * Each P3P pose is scored by angular nearest-blob matching of the facing LEDs (normal within 80 deg of the view
      direction).
    * The best pose goes through Levenberg-Marquardt on the KB4 pixel error, then pixel re-matching, then another
      refinement.
  * **Joint solve:** one `body_from_controller` pose, matched and refined over all four cameras with the factory
    `cam_from_body` extrinsics.
  * **Tracking from a prior:** project the last pose, match within 12 px, refine.
* `pimax-ctrltrack track FILE.pxcam [--fit-extrinsics] [--csv OUT] [--overlay DIR]`:
  * Runs per-frame acquisition in parallel, then a temporal pass.
  * With `--fit-extrinsics`, refits cams 1-3 on the controller poses.
  * Prints the statistics and per-camera bias, and writes a pose CSV and overlay PNGs.
* `pimax-ctrltrack scan FILE.pxcam` prints blob counts per LED frame. `dump FILE REC OUT.png` writes one mosaic.

Results on `~/ctrl-test.pxcam`:

* Full outputs: `controller-6dof/track-summary-fit.txt` and `track-summary-factory.txt`.
* Poses: `controller-6dof/poses-ctrl-test-fit.csv`, in the body frame = OpenXR head axes (x right, y up, z back).
* Per 4 s window: from 12 s to 40 s, most windows have 1.1-1.8 controllers per frame. After 40 s there are none (hands
  lowered, then the headset put down).
* Gaps inside 0-40 s are mostly one controller at the edge of the field of view. The tile edges are where the fisheye
  model is weakest.

Statistics that validate the poses without ground truth:

* Per-camera median blob-minus-projection offset after the refit: cam0-2 within 0.1 px, cam3 -0.28 px.
* Before the refit it was cam2 (-1.5, -0.8) and cam3 4 px.
* Single-camera solves vs the joint pose: median 17 mm. This is expected: depth from one camera of a 9 cm object is
  weak, which is why the joint solve matters.
* Frame-to-frame step at 40 ms: median 7.6 mm. The p99 of 97 mm is fast motion or a wrong frame. A filter with the IMU
  takes care of both.

## 3. Architecture decision

**Use Monado's constellation tracker (`src/xrt/tracking/constellation`, C/C++, about 8.3 k lines) for the live path.**

* **It already models our setup.**
  * Cameras with a fixed pose relative to a moving tracking source: `t_constellation_tracker_camera.pose_in_origin` +
    `has_concrete_pose`, mosaic `tracking_origin` = the HMD pose.
  * LED models with normals and per-LED visibility.
  * Brute-force correspondence search (lambdatwist P3P) and gravity-aided pruning.
  * Pose optimisation (Ceres), blob tracking across frames (`t_rift_blobwatch`).
  * **Per-device IMU fusion** through `imu_sink`, LED-sync refinement, and the device sample callback with
    `m_relation_history`.
* **Proven in production:** `pssense` (PS VR2 Sense controllers) and the Rift CV1 use it today (`target_builder_rift.c`,
  `pssense_driver.c`). We copy the pssense integration pattern.
* **KB4 works unchanged:** the tracker projects through `t_camera_models`, which supports `T_DISTORTION_FISHEYE_KB4`.
  The Pimax driver already fills KB4 calibrations for SLAM.
* **A Rust rewrite would duplicate this** (EKF, prediction, relation history, rerun debug) and still need C glue for
  `xrt_device`. Rust-first is kept where it pays off:
  * `pimax-ctrltrack` is the offline regression and evaluation harness (same recordings, numbers to compare).
  * It is the calibration tool: camera 2 and 3 extrinsics, controller gyro scale and axes, camera-to-IMU time offset.
  * The LED model and calibration can be passed through the existing `pimax-cam` C ABI if that is useful.
* **Rejected alternative:** the full Rust pipeline (blob, ID, PnP, EKF), exported as poses over the C ABI. It is less C,
  but about 1-2 weeks more work before it matches the existing tracker, with no clear gain.

## 4. Open issues

1. **Cam 3 residual.** After a rigid refit it still has 3.8 px RMS. The data cannot yet tell between:
   * (a) a timing offset of the cam-3 LED exposure relative to the others: the error would grow with controller speed;
   * (b) a KB4 intrinsics error at the image edge, where the controllers appear in cam 3: the error would grow with
     radius.

   Test: bin the cam-3 residual by controller speed and by image radius (both available from the pose CSV plus a
   re-projection pass). Meanwhile the tracker can down-weight cam 3 or use a wider tolerance for it.
2. **The extrinsic corrections are not installed anywhere.**
   * Next step: a refit over several recordings, then write them as a `cam-calib-overlay.json` entry for cams 2 and 3
     (format already exists in `pimax-selfcal`; its sanity gate applies).
   * Then cross-check against the selfcal vertical disparity.
3. **Left/right from shape alone is 94 % right.** In Monado, the controller IMU decides it (gyro vs optical rotation
   rate), as pssense does. The left = -x position prior also helps.
4. **Close range under 10 cm** (end of the recording, headset lying on the controllers): not handled. `min_dist` is
   7 cm, and the LED pairs fall outside the 4.5 cm triple window at that range. Low priority.
5. **The gyro scale and axes are still unverified.** One combined recording (section 6) settles them: fit the gyro
   scale, the `ImuToDevice` axes and the time offset against the optical rotation.

## 5. Plan and estimate

| phase | work | estimate |
|---|---|---|
| 1 (done) | Feasibility, LED model, offline blob-ID-PnP slice, extrinsic refit, this document | done |
| 2 | Offline calibration from the combined recording: gyro scale and axes vs optical rotation, camera-to-IMU time offset, cam 2/3 extrinsics (+ cam 3 residual test), written as overlays/config | 2-3 days |
| 3 | Monado (`slam` branch, local only): LED tiles from `pimax_camera.c` (broker source) into 4 `t_rift_blobwatch` -> `t_constellation_tracker`, one mosaic, cameras anchored to the HMD tracking source; `controller_params.json` -> `t_constellation_tracker_led_model`; KB4 calibrations | 3-4 days |
| 4 | Controller IMU into the tracker's `imu_sink` with **device** timestamps (MCU clock -> monotonic via the same offset as the camera), gyro scale from phase 2, `get_tracked_pose` from the relation history; L/R by IMU | 2-3 days |
| 5 | Replay tests: Monado on `ctrl-test.pxcam` plus the HID log, compared with `pimax-ctrltrack` poses; then a live test (hands in front, at the waist, fast swings), latency and jitter numbers | 2-3 days |
| | **total remaining** | **about 9-13 working days (2 weeks)** |

## 6. What the user must do

One combined recording: camera frames and controller reports together, 60-90 s. It gives the gyro scale, the axes and
the time offset, and it is the replay test set for phase 5. The commands were not run by this agent. They open the
headset (camera stream) and listen to the controller reports. `omarchyvr controllers` sends only the attach burst bytes
that Windows sends, as it always has.

```bash
L="flock -o -w 3600 /tmp/omarchyvr-headset.lock"
# terminal 1: camera broker (shares the camera)
$L pimax-cam broker
# terminal 2: camera recording from the broker (2.2 GB per minute)
pimax-cam record ~/ctrl-imu.pxcam --source broker --seconds 90
# terminal 3, started at the same time: every controller report with device timestamps
omarchyvr controllers 95 --raw > ~/ctrl-imu-hid.log
```

During the 90 s, with the room lit as for `ctrl-test`:

1. Press a button on each controller first, so both are awake. Check that terminal 3 prints `controller attached` for
   both.
2. Hold both controllers still in front of the chest for 5 s.
3. Rotate each controller slowly about each of its three axes (roll, pitch, yaw), about 90 deg each way, for about
   20 s. Keep it in front of the headset.
4. Make fast wrist flicks for 10 s.
5. Move the hands left, right, up and down, near (20 cm) and far (arm's length), for 20 s.
6. Hold still again for 5 s, then stop.

Then run the offline part (no device):

```bash
cd ~/omarchyvr/rust && cargo build --release -p pimax-ctrltrack
./target/release/pimax-ctrltrack track ~/ctrl-imu.pxcam --fit-extrinsics --csv /tmp/ctrl-imu-poses.csv
```

The phase-2 gyro and time-offset fit reads `/tmp/ctrl-imu-poses.csv` and `~/ctrl-imu-hid.log`.

## 7. Live MVP (optical first): see the controllers where they are

Built before the full constellation port, so the controllers show up at their real positions in desk mode now.

**Data path:**

1. Camera broker LED frames go to `pimax-ctrltrack live --source broker`.
2. Each frame, it tracks every controller seen before from its prior (about 1-2 ms). If one is missing, it runs the
   brute-force search over the 4 cameras on 4 threads (about 10 ms; only every 3rd frame when nothing has been seen
   for 2 s).
3. It publishes `body_from_aim` per hand to `$XDG_RUNTIME_DIR/pimax-ctrltrack.pose`. This is a 256 B seqlock file;
   the layout is in `src/shm.rs`.
4. Monado (`forks/monado`, branch `desk`, commit d8fc0df, local only; reader `pimax_ctrltrack_shm.h`, with
   `PIMAX_CTRL_TRACK=1`) takes the head pose at the LED frame's time, composes it with that pose, and holds the
   result in the tracking origin.
5. If there is no fix for 0.5 s (`PIMAX_CTRL_TRACK_HOLD_MS`), the controller returns to the old rest pose.
6. Grip = aim times the existing 45 deg grip pitch (pimax-hands).

**Options:**

* `PIMAX_CTRL_TRACK_IMU=1` turns the controller gyro rotation between fixes on. It is off by default because the gyro
  scale and axes are unverified.
* The aim frame is the LED-model frame pitched 52 deg about x (`--aim-pitch-deg`, or
  `PIMAX_CTRLTRACK_AIM_PITCH_DEG`). That is the mean hold in `ctrl-test`, so a controller held naturally points
  straight ahead.
* Camera 1-3 corrections are loaded from `~/.config/omarchyvr/ctrl-cam-extrinsics.json`.
  * They were made by `track --fit-extrinsics --save-extrinsics` on `ctrl-test.pxcam`.
  * A copy is in `controller-6dof/ctrl-cam-extrinsics.json`.
  * `--factory` ignores them.

**Speed-up:** the quartic solver now stops at 1e-9 relative error instead of always running 200 iterations. P3P went
from 83 us to 0.9 us per call. Offline acquisition of all 1492 frames now takes 1.3 s instead of 15 s, with the same
results.

**Offline test (no headset, desk session untouched):**

* Replay broker on a private socket (`pimax-cam broker --source ~/ctrl-test.pxcam --loop --socket S`) feeding
  `pimax-ctrltrack live --source broker:S`.
  * In-view part: left 72-100 %, right 59-78 % of LED frames.
  * Mean work per frame 5-12 ms (25 Hz budget: 40 ms).
* Then `monado-service` (same build) with:
  * `PIMAX_NO_DEVICE=1 PIMAX_CTRL_HANDS_DEMO=1 PIMAX_CTRL_TRACK=1 XRT_COMPOSITOR_NULL=1`;
  * its own `XDG_RUNTIME_DIR`;
  * the `omarchyvr` USB driver disabled in a private `config_v0.json`.

  `analysis/tools/ctrlcli.c` was the OpenXR client.
* Result: aim poses with `POSITION_TRACKED` (flags 0xf), for example L (-0.05 -0.07 -0.38), R (0.11 -0.05 -0.39) m.
  The orientation is near identity when held forward. After 0.5 s without a fix they fall back to the rest pose
  (-0.15 -0.16 -0.38), flags 0x7.
* Known weak spots:
  * At the end of the recording (headset put down on the controllers) a few wrong poses appear.
  * A single new controller far on one side is labelled by that side.

**Launcher:** `omarchyvr desk start --eye --6dof --ctrl-track`.

* It adds `pimax-ctrltrack live --source broker:<socket>` (log `ctrltrack.log` in the session dir) and passes
  `PIMAX_CTRL_TRACK=1` to Monado.
* `desk stop` stops the tracker by its pid.
* `--ctrl-track` without `--6dof` is refused.

## 7b. Out of view: gyro, arm-model hold, reacquire blend (2026-10-14 evening)

The user's live test showed that a controller leaving the camera view fell back to the rest pose after 0.5 s. The pose
logic now lives in Rust: `rust/pimax-hands/src/fuse.rs`, C ABI `include/pimax_ctrlfuse.h`, linked through
`libpimax_capi.a` like the hand code. Monado's `pimax_ctrl.c` (branch `desk`) only feeds it:

* every controller report;
* every optical fix, from a 4 ms poll thread on the pose file, composed with the head pose at the LED frame time.

Monado then asks the fuser for the pose at each requested time.

| situation | what is shown |
|---|---|
| fresh fix (<= 130 ms old) | the fix, position extrapolated with the last velocity, decaying (`v*tau*(1-exp(-age/tau))`, tau 60 ms); orientation rotated by the gyro when calibrated |
| out of view, up to `PIMAX_CTRL_TRACK_LOST_S` (default 4 s) | arm model: the controller keeps its offset in the head-yaw frame (head position + yaw), so it moves with the body and does not swing when you look down; orientation from the gyro (world), else it follows the head yaw |
| longer | the old rest pose |
| reacquire after > 100 ms | blend from what was shown to the optical pose over 100 ms (`PIMAX_CTRL_TRACK_BLEND_MS`) |
| fix that jumps > 35 deg from the gyro prediction, or > 0.15 m + 3 m/s * age from the extrapolation | dropped; the third in a row is accepted |

**Gyro calibration, learned online:**

* The model is `y = C x + d`:
  * `y` is the body-frame angular velocity from two optical fixes about 120 ms apart on one continuous track,
    `log(R_a^T R_b)/dt`;
  * `x` is the mean raw gyro over the same interval.
* It is fitted by least squares (12 unknowns) for 9 camera-to-IMU time offsets (-64..+64 ms); the best R² wins.
* `C` = scale x axis mapping (raw LSB -> aim-frame rad/s); bias = `-C^-1 d`; the bias is refined further at rest.
* After 150 good pairs (at least 40 of them moving, R² >= 0.8) the gyro turns on by itself.
* The fit is saved per hand to `~/.config/omarchyvr/ctrl-imu.json` (every 30 s and at shutdown) and loaded at the next
  start, so the gyro is on from the first second.
* A saved fit is replaced by an online one only after 400 pairs.
* `PIMAX_CTRL_TRACK_IMU=0` turns the gyro off. `=1` forces the old guess (16.4 LSB per deg/s, raw axes) while unfitted.
* The JSON also states the scale in deg/s per LSB and the axis mapping as text, e.g. `"axes": "+y -z -x"`.

**Log for the offline fit:**

* `omarchyvr desk start --6dof --ctrl-track` sets `PIMAX_CTRL_IMU_LOG=<session dir>/ctrl-imu`.
* The driver writes `ctrl-imu-left.csv` and `ctrl-imu-right.csv` (every gyro sample + every fix, first 10 min,
  `PIMAX_CTRL_IMU_LOG_MIN`).
* `pimax-ctrltrack imufit /tmp/omarchyvr-session/ctrl-imu-*.csv [--save]` replays them through the same fit and prints
  R² per time offset. It also prints how well the fitted gyro alone predicts the optical rotation after 0.25 / 0.5 /
  1 s, against holding the last orientation.

**Tests:**

* Unit tests: synthetic 60 s sessions.
  * With 1 deg optical noise, the online fit finds the scale within 3 %, the axes `+y -z -x`, the bias within 3 LSB
    and the +16 ms time offset (R² 0.994).
  * A synthetic driver log, fitted by `imufit` from the file, predicts the 1 s rotation within 1 deg, against more
    than 20 deg when holding the last orientation.
  * Extrapolate, then hold relative to a turning and stepping head, then give up after `lost_s`.
  * The reacquire blend runs 0 -> 50 % -> 100 % over 100 ms; an outlier 1 m away is dropped.
* Replay through Monado: the same setup as section 7 (private runtime dir, no device, ctrl-test.pxcam through a
  private broker), 60 s of `ctrlcli`.
  * About 659 fixes per hand reached the fuser. 1-2 were dropped as outliers and 21-25 reacquires were blended.
  * Rest pose: 20-26 % of the time, which is the long hands-at-the-waist part at the end of the recording. With the
    old 0.5 s rule it was 43-45 %.
  * The out-of-view hold covered 27-32 %.
  * The recording has no controller HID data, so the gyro path is covered only by the synthetic tests until the
    user's next live run.

## 7c. Hands that merged or jumped together: diagnosis and body model

**Evidence** (user's live session, `/tmp/omarchyvr-session/ctrl-imu-left.csv`, 900 fixes): the left hand's optical
fixes jumped 20-40 cm onto the right controller's place for one or two frames and then came back (for example at
1.6 s, 7.5 s and 32.9 s: x -0.20 -> +0.08 -> -0.22 m, head frame). That is identity confusion. The live tracker gave a
second solve, or the other controller, to the slot that was free:

* one candidate always went to the only free slot;
* two candidates were split only by their x order.

The fuser's position gate (0.15 m + 3 m/s * age = 0.27 m at 40 ms) let most of these jumps through. The "third
outlier in a row is accepted" rule did the rest.

Side finding: `ctrl-imu-right.csv` stopped after one line. A stale right-hand fix from an earlier tracker run was
still in the pose file and anchored the log's 10-minute end time in the past. Now the C feed thread skips fixes older
than 2 s, and the fuser drops stale ones. There was no evidence that the head-relative hold switching both hands at
once caused it.

**Fix: a rule-based body model in both places.**

* **Tracker** (`rust/pimax-ctrltrack/src/assign.rs`, used by `live`; `--no-body` gives the old behaviour for A/B):
  * Search results are assigned to the free slots by joint cost: distance to the slot's last pose, wrong-side
    penalty, shape label.
  * Hard limits:
    * reach 0.8 m from a shoulder (+-0.18, -0.25, +0.10 m from the eyes);
    * a hand at most 0.3 m across the head centre;
    * continuity of 0.12 m + 2.5 m/s * age for a slot seen within 1.5 s;
    * never within 8 cm of the other hand.
  * Prior-tracked poses that break these rules are dropped. Two tracked poses on one controller: the one that moved
    more is dropped.
* **Fuser** (`rust/pimax-hands/src/fuse.rs`; the two hands are linked through `pimax_ctrlfuse_link`):
  * Fixes out of reach (0.75 m + 5 cm), on the wrong side, within 8 cm of the other hand's fresh fix, or stale are
    always dropped. There is no "accept after 3" for these.
  * A jump vs the prediction is believed only after 4 in a row (a turn vs the gyro after 3).
  * Extrapolated and held poses are kept inside the reach sphere and at least 8 cm from the other hand, pushed
    outward.
  * Re-entry blending is unchanged.

**A/B on the replay** (both trackers on the same broker frames, 62 s):

| | poses L/R | teleports > 15 cm in one frame | hands < 8 cm apart |
|---|---|---|---|
| old | 716/673 | 12 | 12 |
| new | 711/665 | 2 | 0 |

The 2 teleports left fit within 2.5 m/s, so they may be real fast motion.

Through Monado (no-device replay):

* 4 fixes dropped as "merged";
* 0 of 300 output samples with the hands closer than 7.5 cm.

**Log for a learned motion prior later.** Per hand, `ctrl-imu-*.csv` now has:

* `I`: gyro + accelerometer raw, about 500 Hz;
* `O`: every optical fix, with the head pose and the verdict (ok / reach / side / merged / stale / jump / turn);
* `P`: the shown pose, head pose and the other hand's position, at up to 50 Hz.

## 7d. Session 18:13 follow-up: robust gyro fit, blend timing, velocity (2026-10-14)

**Session 18:13** (logs in `~/.local/share/omarchyvr/ctrl-sessions/20261014-1813/`):

* Coverage: optical 56-59 %, extrapolated 6 %, out-of-view hold 35-38 %, rest pose 0 %.
* Merges: 0-1. Dropped as "merged": 13 + 11.
* The gyro fit never qualified (plain least squares, R² 0.33 / 0.41).

**Fixes:**

1. **Robust gyro fit.**
   * The fit keeps the pairs (up to 3000) and runs least squares plus 4 rounds that drop residuals above the 85th
     percentile. Only fixes with 9+ LEDs are used.
   * On the 18:13 logs:
     * left: R² 0.990, 0.1221 deg/s per LSB, axes `-z -x +y`;
     * right: R² 0.990, 0.1202 deg/s per LSB, axes `+z +x +y`;
     * time offset 0 ms for both.
   * The gyro alone predicts the optical orientation after 0.25 / 0.5 / 1 s within 2.0-2.5 / 3.2-3.4 / 3.8-4.0 deg
     (median). Holding the last pose gives 13-19 / 25-35 / 24-45 deg.
   * Saved to `~/.config/omarchyvr/ctrl-imu.json` with `pimax-ctrltrack imufit <logs> --save`, so the gyro is on from the
     next start.
2. **The re-entry blend starts at the first display request after the fix**, not at the LED frame time (40-80 ms
   earlier). It blends from what that request would have shown and lasts 150 ms.
3. **Velocity:** an EMA over about 3 fixes, capped at 1.5 m/s; extrapolation 80 ms.

## 7e. Session 18:27: where the remaining motion comes from, filter (2026-10-14)

Logs: `~/.local/share/omarchyvr/ctrl-sessions/20261014-1827/`. The gyro was on (saved fit), and `ctrltrack.log` shows
that the camera 1-3 extrinsic corrections were loaded.

Metrics: `analysis/tools/ctrl_motion.py`, on the live log and on `pimax-ctrltrack fusereplay` output (same gyro
samples, fixes and display-request times, replayed through the fuser).

**Where the motion comes from:**

* **Jitter at rest depends on the LED count.**
  * Fix-to-fix step with the controller still: 10+ LEDs 0.4-0.6 mm, 5-6 LEDs 7-34 mm.
  * Shown pose in the world: p50 12 mm, p90 41-48 mm.
  * In the head frame the optical fixes jitter only 2.4-3.7 mm (p50). So part of the world-frame motion is the
    head's own 6DoF pose corrections.
* **Camera handovers:** not separable. The log has no camera count per fix, and the extrinsics did load. LED-count
  changes of 3+ coincide with 7-39 mm steps, so this is mostly the few-LED noise above.
* **Flipped few-LED solves.**
  * 157 + 162 fixes were rejected as "turn" (LED count p50 7); some were accepted after 3 in a row.
  * 23 + 27 orientation flips over 90 deg were shown within 30 ms.
  * Over out-of-view gaps the gyro agrees with the optical rotation within 1-6 deg, except where one end is a flipped
    5-6 LED solve (120-180 deg).
* **Re-entry:**
  * 89 + 122 shown steps over 50 mm. Part of that was a logging artefact: the shown pose was logged before the blend.
  * Position error at re-entry: p50 113-191 mm (held vs the optical pose).

**Changes in `fuse.rs`:**

1. **Filter.** Alpha-beta position and velocity with gains by LED count: 0.2 (<=6 LEDs), 0.5 (7-9), 0.85 (10+),
   halved above 1.5 px RMS. The orientation is carried by the gyro and pulled toward the optical one; gains are x0.35
   at rest (One-Euro style). The gyro fit still uses the raw optical poses.
2. **Few-LED solves (< 9 LEDs) that disagree with the gyro by more than 35 deg** now give their position only; the
   orientation stays from the gyro ("pos-only"). The gyro check also covers re-entries up to the 4 s lost time.
3. **Out of view and still** (gyro < 0.2 rad/s): the controller is held in the world instead of relative to the head.
   It re-anchors without a jump when it starts or stops moving.
4. The shown pose is now logged after the blend.

**Replay A/B on this session** (live = what was shown, new = replay; left / right):

| metric | live | new |
|---|---|---|
| world jitter at rest p50 / p90 (mm) | 12 / 48, 12 / 41 | 8 / 20, 14 / 45 |
| rotation jitter at rest p90 (deg) | 0.65, 0.46 | 0.21, 0.08 |
| orientation flips > 90 deg shown | 23, 27 | 0, 0 |
| samples shown as optical | 2397, 2321 | 2540, 2519 |
| hold -> optical steps > 50 mm | 89, 122 | 32, 40 |
| re-entry orientation error 1-2 s p90 (deg) | 163, 124 | 165, 26 |

The right hand's world jitter at rest did not improve. The head-frame jitter there is about 2-4 mm, so the rest comes
from the head pose.

**Not done:** an EKF that uses the accelerometer for position (the gyro and the optical poses already carry the
orientation); logging the camera count per fix.

## 7f. 2026-10-16: gyro calibration confirmed, masked replay, forearm model

Full numbers: `analysis/linux-recordings-20261016.md`.

* **Gyro calibration (the missing phase-2 step):** 0.122 deg/s per LSB, axes left `-z -x +y`, right `+z +x +y`,
  8-11 deg mounting rotation, time offset 0 / 4-8 ms, the same in all four logs of both hands (take B and the
  10-14/15 sessions); the accelerometer shares the gyro axes. The saved fit is now also built in
  (`fuse::default_calib`), so the gyro is on without `ctrl-imu.json`; `PIMAX_CTRL_TRACK_IMU=0` turns it off.
* **Masked replay** (`pimax-ctrltrack occlusion`): visible stretches are hidden for 0.5 / 1 / 2 s and the fuser's
  output is scored against the hidden fixes. Orientation from the gyro: about 2 deg p50 for 2 s (18-36 deg without).
  Position, before -> now (p50): 51 -> 42 / 89 -> 69 / 143 -> 110 mm (left), 48 -> 42 / 68 -> 56 / 88 -> 75 mm
  (right); re-entry snaps p90 19-30 mm (were 29-42).
* **Changes in `fuse.rs`:** forearm model (the elbow is held, the controller swings around it with the gyro;
  `PIMAX_CTRL_TRACK_FOREARM`), the hold follows half the head yaw (`PIMAX_CTRL_TRACK_FOLLOW_YAW`), re-entry at most
  0.5 m/s and 120 deg/s (`PIMAX_CTRL_TRACK_BLEND_MPS`), rest bias no longer learned from a steady turn. Accelerometer
  dead reckoning was tried and is worse at every setting (off, `PIMAX_CTRL_TRACK_DR_MS`).

## 8. Reproduce phase 1

```bash
cd ~/omarchyvr/rust && cargo build --release -p pimax-ctrltrack && cargo test -p pimax-ctrltrack
./target/release/pimax-ctrltrack scan ~/ctrl-test.pxcam | tail          # blob counts per LED frame
./target/release/pimax-ctrltrack track ~/ctrl-test.pxcam                # factory extrinsics
./target/release/pimax-ctrltrack track ~/ctrl-test.pxcam --fit-extrinsics --csv /tmp/p.csv --overlay /tmp/ov --overlay-every 60
```
