# 6DoF ideas for the Dream Air on Linux (2026-10-14)

Research pass for the Monado + Basalt tracker. Builds on `6dof-head-tracking.md`, `room-model-world-lock.md`,
`selfcal-roommap.md`, `windows-6dof-params.md`, `slam-exposure.md` and `ae-stall.md`. Nothing run on the device.

Tags: **[L]** = literature or repo fact seen in a source (fetched or search snippet). **[H]** = my own hypothesis or new
combination, not tested anywhere. **[U]** = could not verify (paywall, bot wall, or missing from search).

## 0. Symptoms, and which ideas target them

| Symptom (from our notes) | Likely layer | Ideas |
|---|---|---|
| 2.2 m drift while standing still in the first 5 s | init (bias, gravity, scale) or noise model | A1, A2, A3 |
| 23 % loop drift on a walk | scale observability, time/extrinsics, noise model | A3, A4, A6 |
| Overexposed frames (23-32 % saturated) | front end | A7, A8 |
| "Not enough matched features" (163x on Windows too) | front end, camera coverage | A7, A9 |
| Vertical drift and jumps after headset off/on (Windows) | re-init, floor reference | A5, A10 |

## 1. Ideas

### A1. Stationary-start gate with zero-velocity update (ZUPT) and bias/gravity init from the still window
- **What:** While the IMU looks still (low gyro and accel variance over about 0.5 s), zero the velocity, average the
  accel for gravity and the gyro for bias, and hold the pose. Leave the still state only when the motion detector fires.
  ZUPT is a standard EKF measurement update [L: hybrid error-state EKF/UKF, arXiv 2512.17505; ZUPT + gravity alignment
  in quasi-static periods]. OrcVIO detects pseudo-zero inertial measurements and checks velocity magnitude before
  applying bias residuals [L: arXiv 2007.15107]. ModalAI's VOXL OpenVINS init does a static path (average accel for
  gravity, zero velocity, bias priors) and sends tilted starts to dynamic init [L: docs.modalai.com/voxl-open-vins-init].
- **Why it fits:** 2.2 m of drift in 5 s with the user standing still is a velocity/bias or init problem, not slow
  integration. A static gate removes that whole class of error. Pimax's own log says "imu initial gravity estimator is
  good", so gravity is fine. The missing piece is the velocity and bias during the hold.
- **Repo:** arXiv 2512.17505, arXiv 2007.15107, docs.modalai.com/voxl-open-vins-init. Basalt's own static init: **[U]**
  not confirmed (search did not return the Basalt init code). Check `basalt/src/vi_estimator` before writing ours.
- **Effort:** S (a gate in our Rust supervisor plus a zero-velocity hold in the pose output; no Basalt patch needed
  for the hold). M if it must go inside Basalt's estimator.
- **Test first:** replay the stationary 5 s of `20261014-152215-live` offline with the gate on and off. Expect
  about 0 m over 5 s if init is the cause. [H] This is my hypothesis: the 2.2 m comes from velocity and bias
  settling during the first seconds, not from the standing user moving.

### A2. Allan-variance IMU noise model from our own stationary recording
- **What:** Record 1-2 h with the headset still (IMU only, no tracking), run `imu_utils` (gaowenliang) or
  `allan_variance_ros` / `allan_ros2`, and read white noise N, random walk K and bias instability B. Kalibr
  consumes the YAML. OpenVINS advises inflating the values by about 10-20x for unmodelled error [L: OpenVINS calibration
  guide; imu_utils forum notes]. Note: gaowenliang/imu_utils reportedly prints N and B but not K, and its units in the
  README are wrong [L: forum post, search snippet]. Check before use.
- **Why it fits:** our defaults `accel 4e-3`, `gyro 4e-4` are tighter than Pimax's own stationary values from the
  older calibration (`gyro 0.00197`, `accel 0.0817`) [L: `windows-6dof-params.md`]. Over-confident gyro makes the
  filter trust bad rotation. Pimax's active XML has all-zero `IMUNoise`, so it does not tell us the truth.
- **Repo:** github.com/gaowenliang/imu_utils; github.com/matthewwarnes/allan_ros2; OpenVINS docs.
- **Effort:** S (an hour of recording plus an offline run). Fits our EuRoC-style recording.
- **Test:** rerun the replay with the measured N and K inflated 10x and 20x. Compare closure error on the walk.

### A3. Camera-IMU time offset estimated online, cross-checked offline
- **What:** The offset between camera and IMU clocks can vary over time. TON-VIO models it with learned networks, and
  says Kalibr measured swings of up to 5 ms offline [L: arXiv 2403.12504]. Classic methods add the offset to the VIO
  state and re-time IMU data [L: Robotica 2024 double-stage EKF; VINS-Mono online temporal calibration, arXiv
  1808.00692]. The Monado SLAM dataset estimates clock offsets by aligning gyro rotational velocity with a
  visual-only rotation rate, following TUM-VI [L: arXiv 2508.00088, as summarised by a fetch; numbers not retrieved].
- **Why it fits:** the factory `delta` is 1.638 ms in the active XML and 2.444 ms in the older one (Windows note), and
  our `slam-exposure.md` already says the mid-exposure stamp shifts frames by 5.2 ms. A few ms of error at 1 kHz IMU
  gives a constant bias in the gyro-camera pairing, which shows up as a slow rotation drift.
- **Repo:** arXiv 2403.12504, arXiv 1808.00692, github.com/HKUST-Aerial-Robotics/VINS-Mono (online td in the
  estimator).
- **Effort:** S for an offline sweep with the existing `timing.rs` / `shift_cams` tooling (we already have a sweep
  framework; add gyro-rate cross-correlation as a second method). M for an online state in Basalt.
- **[H]:** the gyro cross-correlation on our `.pxcam` recordings gives an independent estimate. If it lands near
  1.6 ms, the factory value is confirmed and the 2.2 m drift is not timing.

### A4. Metric-scale observability check, and a walk that excites it
- **What:** Scale is observable only when the motion is excited. A lightweight excitation metric from raw IMU
  (rank of the observability matrix, gravity-acceleration asymmetry) predicts it. Reported scale error: straight-line
  motion 9.2 %, circular 6.4 %, figure-eight 4.8 % [L: arXiv 2603.26740, monocular VIO].
- **Why it fits:** our 23 % loop drift came from a walk that was largely straight. Scale error of this size matches
  the 23 %. The metric is computed from IMU alone, so it costs nothing at runtime.
- **Repo:** arXiv 2603.26740 (no code link seen).
- **Effort:** S (offline metric on existing EuRoC recordings; a walk protocol with turns and a figure-eight).
- **[H]:** our walk excitation score is low, and the figure-eight protocol reduces loop drift by a large factor. This
  is cheap to test before any code change, so do it first.

### A5. Floor-height prior and z clamp, from the stationary start pose
- **What:** Head height above the floor is roughly constant in a session (the seated or standing user). On the
  first stationary init, record the head height relative to the floor plane (or the gravity-aligned z at rest) and
  clamp or softly penalise z excursions. Room-scale systems use a floor level set at start (Vrgineers doc: place the
  headset on the floor and calibrate) [L: vrgineers portal; SteamVR forum]. A SteamVR user reports the floor jumping a
  few inches on tracking reacquisition [L: forum snippet, weak].
- **Why it fits:** Windows drifts through the floor or ceiling, which is a z-only error. A z prior bounds it directly
  and needs no feature matching.
- **Repo:** none found with code. Idea only.
- **Effort:** S (a z soft-constraint in the pose output; then a check on the Monado side that the origin is not
  moved). M if done as a Basalt prior factor.
- **[H]:** the Windows vertical drift is the same failure as our standing drift, seen only in z. The floor prior would
  fix both if A1 has not.

### A6. Headset-removed state machine: freeze, re-anchor, then relocalise
- **What:** Use the proximity sensor (or a sudden IMU and image-brightness change, plus loss of features) to switch to
  a "doffed" state: freeze the pose and stop feeding the tracker. On re-don, run a short stationary init (A1) and
  relocalise against the session map before trusting the VIO pose. Pimax does load a saved map on start and saves on
  exit (`platMap_orborb`, `loc_mode_ 0`, log: "load map to reloc, and save map") [L: `windows-6dof-params.md`].
- **Why it fits:** the Windows vertical jump after doff/don is a re-init that takes a new arbitrary origin. Freezing
  and re-anchoring keeps the world frame.
- **Repo:** ORB-SLAM3 Atlas save and load (arXiv 2007.11898; docs conflict on SaveMap/LoadMap, check source) [L].
  Basalt map save and load: **[U]**, not found in search. DM-VIO and OKVIS2 2025 map features: **[U]**, nothing found.
- **Effort:** M (state machine in the Rust supervisor; relocalisation needs ORB or DBoW2 place recognition, which is
  L).
- **[H]:** a short stationary init plus floor-prior re-anchor is enough for doff/don, without full relocalisation.
  Test it before building relocalisation.

### A7. Exposure-robust front end: fix the photometry before the detector
- **What:** Two cheap steps. (a) Run AE (already in progress, `0x51`). (b) Normalise each patch or frame (local
  contrast or gain-compensated intensity) before KLT or feature detection. Learned matchers are more tolerant of
  dim or overexposed input than classic detectors [L: SuperVINS arXiv 2407.21348; LIR-LIVO arXiv 2502.08676; both
  report qualitative robustness, not controlled tests].
- **Why it fits:** 23-32 % of pixels were saturated in the lit walk. Saturated patches have no gradient, so features
  are lost (the "not enough matched features" warnings).
- **Repo:** github.com/magicleap/SuperPointPretrainedNetwork; github.com/cvg/LightGlue; github.com/verlab/accelerated_features
  (XFeat). Basalt uses its own KLT patches, so this is an L change.
- **Effort:** S for (a) (done); S for (b) as a preprocessing step in the broker. M for XFeat or SuperPoint+LightGlue
  in the front end (CPU cost at 2 x 25 Hz is the open question; XFeat has 64-dim descriptors, SuperPoint 256 [L:
  arXiv 2607.04615 summary]).
- **[H]:** gain-compensated normalisation before KLT lifts feature counts in the saturated segments with no learned
  model. Test on `pimax-slam` replay.

### A8. Low-cost matcher on a downscaled pyramid
- **What:** Run SuperPoint or XFeat on a 320x240 copy of each tracking frame, and feed the matches to Basalt as
  feature tracks. XFeat is designed for low compute.
- **Why it fits:** the "not enough matched features" count (about 21-26 per run, 163 times) is a matcher-coverage
  issue.
- **Repo:** arXiv 2607.04615 (XFeat comparison); benchmark arXiv 2603.18589 (SuperPoint, XFeat, LightGlue under
  challenging light; no clear winner seen in the snippet).
- **Effort:** M (feature-track injection into Basalt is not a supported API; check the VIT interface).
- **[H]:** this only helps if A7 is done first, because the features it adds are in the saturated areas.

### A9. Use all four cameras, or the other two tiles, for the divergent layout
- **What:** Multi-camera VIO with non-overlapping fields of view, where each camera adds features in its own region
  and the back end is shared [L: arXiv 2109.12030, helmet with two fisheye pairs; VINS-Multicam, CCDC 2025 (ASLfeat
  front end, cross-camera loop closure); Sphere-VIO arXiv 2606.29910 (heterogeneous multi-camera; the snippet mentions
  unstable scale with non-overlap; check the date). Meta's headset patents US10078218 and US10095031 cover
  non-overlapped stereo for headset tracking [L: patents, not papers].
- **Why it fits:** we track with 2 of 4 cameras. The other pair could cover the floor or ceiling and fix z (A5) and
  reduce the feature-starved periods. Pimax says 4 cameras; our notes say 2 lower tracking cameras. Check which tiles
  are tracking before this.
- **Repo:** Basalt multi-camera config (already supports N cameras in principle, **[U]** whether the VIT interface
  passes all four). Confirm first.
- **Effort:** L (calibration of the divergent pair, extrinsics against the IMU, and the Monado VIT wiring).
- **[H]:** the two tracking tiles and the other two are not an overlapping stereo pair. Their extrinsics must come
  from the Pimax XML `Rig` entries (cam A identity, cam B given), so the other two need the same treatment.

### A10. Learned relocalisation or place recognition (hloc-style) for map reuse
- **What:** Match the current view to a saved keyframe database with global descriptors, then solve PnP against the
  stored 3D points. This is the standard way to relocalise after tracking loss. Pimax's own ORB map has 3 submaps and
  278/132/93 keyframes [L: `windows-6dof-params.md`], so map reuse is an existing product feature.
- **Why it fits:** the Windows re-don drift. Also covers the first-minute start with a known room map (see
  `room-model-world-lock.md`).
- **Repo:** github.com/cvg/Hierarchical-Localization (hloc). Pimax's `voc_GEN_8X4.dbow` is not on the shared drive
  (note in `windows-6dof-params.md`), so we cannot reuse their vocabulary.
- **Effort:** L.
- **[H]:** we can build our own room map from our own keyframes (already a plan in `selfcal-roommap.md`). Use A6 first
  and only build this if A6 is not enough.

### A11. Pimax's Windows noise and calibration as a prior, not a value
- **What:** Use Pimax's older stationary noise values only as an upper bound for our noise model, not as truth.
- **Why it fits:** their active file is all zero, so the old file is the only data point, and it may be wrong too.
- **Effort:** S.

## 2. What I could not verify

- **WMR and Rift S Basalt configs.** The noise fields exist in Monado's struct docs: `wmr_inertial_sensor_config`
  has `noise_std` and `bias_var`; `rift_s_imu_config_info_t` has `imu_hz`, scale, temperature fields [L: monado docs].
  The actual numbers and the Basalt JSON keys were not retrieved. Next step: read `src/xrt/drivers/wmr` and
  `src/xrt/drivers/rift_s` in the Monado tree, plus the Basalt MR 4 (`mateosss/basalt`), which returned an Anubis
  bot-wall page. Open it in a browser or fetch it locally with `git clone`.
- **Basalt map save and load, static init, and relocalisation.** Not confirmed from search. Read
  `basalt/src/vi_estimator` and `basalt/src/...` in the local `~/src/basalt` tree.
- **DM-VIO and OKVIS2 2025 releases.** Nothing new found.
- **Numbers in the Monado SLAM dataset paper** (arXiv 2508.00088): the calibration procedure is summarised above, but
  the Basalt ATE table is only in the full PDF. Fetch the PDF locally if we need numbers.

## 3. Top-5 ranked action list

1. **A2 + A4 (S, offline, today):** measure our IMU noise (Allan, 1-2 h still recording) and check the walk's
   excitation score. Both use existing recordings or a single new one. Cheapest test of the drift cause.
2. **A1 + A5 (S-M):** stationary gate with zero-velocity hold and a floor-height z prior, replayed on the 5 s still
   start. Targets the 2.2 m standing drift and the Windows vertical drift together.
3. **A3 (S):** gyro-rate cross-check of the camera-IMU time offset on the existing `.pxcam` files. Confirms or
   removes the 1.6 ms vs 2.4 ms question.
4. **A7 (S):** gain-compensated patch normalisation before KLT, then a replay on the saturated segments.
5. **A6 (M):** headset-removed state machine (freeze, short stationary init, floor re-anchor). Defer true
   relocalisation (A10) until this has been tested.

Later, if the top 5 do not close the gap: A9 (use the other camera pair, L), A8 (learned matcher, M).
