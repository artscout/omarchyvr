# 6DoF head tracking for the Dream Air on Linux: design and phase 1 (2026-10-14)

Status: phase 1 implemented and tested offline (no headset run yet). Monado worktree `forks/monado`, branch
`slam`; Rust in `rust/pimax-cam` (module `slam`, C ABI `capi_slam`) and the new tool crate `rust/omarchyvr-slam`;
launcher `analysis/tools/run_slam.sh`. Basalt is built in `~/src/basalt` and installed in `~/.local`.

## 0. Short answer

* **Recommendation: Monado's SLAM tracker with Basalt** (the `mateosss/basalt` fork, loaded at run time through
  the VIT interface), fed by the two lower tracking cameras at 25 Hz and the IMU at 1 kHz. It is BSD-3, the
  Monado developers maintain it, it handles KB4 fisheye stereo plus IMU, and other inside-out drivers (WMR, Rift S,
  North Star) already use it the same way. ORB-SLAM3 is the fallback if relocalisation turns out to matter more
  than latency; a VIO of our own in Rust is not worth it now (section 2).
* **Phase 1 status:** `libbasalt.so` built as the user (no patches, no system packages). The driver feeds cameras
  and IMU on one clock into `t_slam` behind `PIMAX_SLAM=1` (default off). The calibration comes from the factory
  XML through Rust. A Rust supervisor decides when to trust the tracker and falls back to 3DoF. Any session can
  record a EuRoC dataset (`PIMAX_SLAM_RECORD`), even without Basalt. Offline replay
  (`run_slam.sh replay`, i.e. `monado-cli slambatch`) runs end to end: Basalt loads our KB4 calibration,
  initialises and writes a trajectory. Build and ctest pass (28/28), and the Rust tests pass.
* **Biggest risk found: camera exposure.** On Linux nobody sends the auto-exposure command. Every tracking frame
  runs at a fixed exposure/gain of 10410/449 (raw), in bright and dark rooms alike. On Windows the host pushes
  exposure at 12.5 Hz (command `0x51`, about 3143/13). The tracker needs this fixed: auto-exposure is the first
  phase-2 item (section 5).
* **Not proven yet:** accuracy. Every existing `.pxcam` clip is camera-only (no IMU) and the long one was shot in
  the dark, so the first live step is a 90 s lit recording (section 9).

## 1. What the sensors give us (measured)

| Item | Value | Source |
|---|---|---|
| Cameras | 4x OV7251 mono 640x480, KB4 fisheye (fx ~232), one UVC stream 2560x832 grey | camera-frames.md, calibration XML |
| Frame pattern | 50.03 fps total, strictly alternating **T**racking / **L**ED exposure | `omarchyvr-slam timing` |
| Tracking rate | **25.02 Hz** per camera (39.976 ms); all four tiles share one timestamp | timing, clip 2026-10-14 00:14 |
| LED frames | exposure/gain 40/62: dark, controller LEDs only, useless for SLAM | timing |
| Tracking exposure on Linux | **fixed 10410/449** on all tiles, no AE | timing (all three recordings) |
| Frame timestamp | u32/u64 µs in the metadata row, **the IMU's MCU clock** | camera-frames.md §Clock relations |
| Delivery | frames arrive ~12 ms after their stamp; jitter above the fastest frame median 1.95 ms, p95 3.8 ms | timing, camera-frames.md |
| Drops | 44 lost of ~1075 in 21 s, one 540 ms hole (USB hub resets are known) | timing |
| Host vs MCU clock drift | about -7 µs/s | timing |
| IMU | 1 kHz (500 reports x 2 samples), accel 2048 LSB/g, gyro 16.4 LSB/dps | imu-camera.md |
| IMU axes | raw x,z negated = OpenXR head axes (`PIMAX_AXIS_MODE=1`) = the factory "body" frame | imu-orientation.md, pimax-cam calib.rs |
| Extrinsics | `ombc`/`tbc` (cam0 -> body) + `<Rig>` per camera; trackingA/B baseline 11.2 cm along body x, looking forward and ~29 deg down, mounted upside down | calib tests |
| Camera-IMU time offset | factory `delta` = 1.638 ms (sign/semantics unverified) | XML |
| IMU noise | factory `IMUNoise` all 0.0 | XML |

Calibration quality: trackingA/B agree to about 0.2-0.6 deg. The upper "controller" cameras disagree with them
by 3-15 px after rectification (selfcal-roommap.md §1), so they are unproven for metric SLAM.

## 2. Options

| | (a) Basalt via Monado VIT | (b) ORB-SLAM3 via Monado's adapter | (c) own Rust VIO |
|---|---|---|---|
| Licence | BSD-3 | **GPLv3** (fine for private use; any binary we ship with it becomes GPL) | ours |
| In Monado | first-class (`t_tracker_slam.cpp`, dataset tools, prediction, UI); WMR, Rift S, North Star, RealSense | older adapter; Monado's docs now point to Basalt | none |
| KB4 fisheye + stereo + IMU | yes (`kb4` camera model, VIT KB4 calibration) | yes (KannalaBrandt8, stereo-inertial) | to write |
| Speed | fast: sqrt marginalisation, optical-flow front end, 1 thread is enough for 2x 640x480 at 25 Hz | heavier (ORB per frame, local BA) | unknown |
| Relocalisation / saved map | no map save/load in the VIT fork; VIO with a sliding window, recall of lost points | **yes**: Atlas, multi-map, DBoW2 place recognition, map save/load | to write |
| Robustness to fast motion | good with IMU at 1 kHz | OK; loses track more often, then relocalises | unknown |
| Effort | phase 1 done | build GPL fork + vocabulary + adapter tuning | months |
| Pimax itself | - | Pimax's Windows tracker is ORB-SLAM-style (map `platMap_orborb_K8L4.bin`, DBoW vocab) | - |

**Choice: (a).** It is the path Monado maintains, it ran on the first try against our Monado and our calibration,
and its licence matches. Pimax's choice of ORB-SLAM says relocalisation and room maps matter. We get those as
our own layer on top of (a), in Rust (section 8). If that layer turns out harder than expected, (b) can be swapped
in behind the same VIT interface without driver changes, because the driver only knows `t_slam`.

## 3. Architecture (phase 1, implemented)

```
libpimax session thread (IMU 1 kHz) --pimax_on_imu--> map axes (mode 1), m/s^2, rad/s, IMU-clock ns
    |-> 3DoF fusion (m_imu_3dof) -> relation history (as before)
    '-> pimax_camera_push_imu -> t_slam IMU sink  and/or  EuRoC recorder IMU sink
pimax-cam capture thread (UVC 50 fps) -> tracking frames only (25 Hz)
    -> one L8 640x480 xrt_frame per needed tile, shared read-only:
         Mercury (PIMAX_HANDS=mercury), t_slam cams[PIMAX_SLAM_CAMS], EuRoC recorder cams
    -> after each set: t_slam_get_latest_pose -> Rust supervisor (pimax_cam_track_update)
HMD get_tracked_pose(t) -> 3DoF relation -> pimax_camera_head_relation:
    TRACKING: t_slam pose at t (IMU dead reckoning past the last VIO pose), z-up -> y-up,
              * world_from_slam (yaw + translation from the supervisor) * head offset
    otherwise: 3DoF orientation * heading fix, at the held position
```

* **Clock.** Camera metadata and IMU share the MCU µs clock. The HMD keeps one device->monotonic mapping
  (minimum-delay filter, `sample_time_ns`) and publishes it (`pimax_hmd_clock_ref`). Frames are mapped with the
  same reference (`pimax_cam_dev_to_mono_ns`), so camera and IMU land on one timeline and USB jitter (p95
  3.8 ms) never enters the tracker. Frames without that reference (replays, before the first IMU sample) are not
  given to the tracker (`slam: ... untimed frames skipped` in the log).
* **IMU frame and extrinsics.** We feed the IMU in the factory body frame (raw axes with x,z negated, which the
  driver already uses for 3DoF), so `T_imu_cam = body_from_cam_i` comes straight from `ombc/tbc` and `<Rig>`
  (`pimax_cam::slam::t_imu_cam`; tests check the 11.2 cm baseline along body x and the forward optical axes).
  Samples go in raw (not bias-corrected); Basalt estimates the biases.
* **Calibration hand-over.** Two ways, same numbers:
  1. driver -> `t_slam_calibration` (KB4 intrinsics, `T_imu_cam`, 25 Hz, IMU noise) when `SLAM_CONFIG` is unset;
     Basalt then uses its built-in VIO defaults;
  2. `omarchyvr-slam export` -> `pimax-basalt-calib.json` (Basalt cereal, `camera_type: kb4`) +
     `pimax-basalt-config.json` (Basalt's EuRoC config, detection grid 50 -> 40 for 640x480) +
     `pimax-basalt.toml` (SLAM_CONFIG, absolute paths, GUI off). `run_slam.sh live` uses this, so live and
     offline runs use identical settings.
* **IMU noise.** The factory values are 0. Defaults (`pimax_cam::slam::ImuNoise`): accel 4e-3 m/s²/√Hz,
  gyro 4e-4 rad/s/√Hz, bias walk 1e-3 / 1e-4. That is datasheet consumer-MEMS density x4-5, the same order as
  Monado's hard-coded Rift S numbers. `omarchyvr-slam imu-noise <recording>` measures the white noise in the stillest
  2 s window (inflated x3) and writes `~/.config/omarchyvr/slam-imu.json`, which both the driver and the exporter pick
  up. Bias random walk needs minutes of still data (Allan deviation), so it stays at the defaults for now.
* **Head offset.** The tracker reports the IMU pose. The head origin is put between the two tracking cameras,
  5 cm behind them (the same estimate as the Mercury hand attachment; `PIMAX_HT_HEAD_OFFSET` overrides it). The
  linear velocity includes ω x r for that lever arm.
* **Monado changes outside the driver:** `t_slam_get_latest_pose()` (the newest VIO pose and its time, with no
  prediction) and `t_slam_reset()` (exposes VIT `tracker_reset`, previously only a debug-UI button).

## 4. Which cameras, which rate

* **Phase 1: trackingA + trackingB (ids 0,1), 25 Hz.** They form the best-calibrated pair, a forward stereo rig
  with 11 cm baseline and enough overlap for metric depth at start-up.
* **All four (`PIMAX_SLAM_CAMS=0,1,2,3`)** is wired (the tracker takes up to 5) and needs no new code. The upper
  cameras widen the field of view (looking down at a desk, up at a ceiling) but carry the 3-15 px disagreement.
  Turn them on only after the selfcal overlay is trusted, and compare offline first (the same recording with 2
  and with 4 cameras, section 9).
* **Frame rate.** 25 Hz is what the firmware gives for tracking exposure (the other 25 are LED frames). It is
  low but workable: Rift S runs Basalt at 30 Hz. The IMU bridges the gaps, and prediction to display time uses
  IMU dead reckoning (section 7). `update_fsin_pwm current_hz 50` in the firmware log suggests the 50 Hz sync is
  host-selectable. A 60 Hz mode (30 Hz tracking) exists as a UVC interval and is worth a test once exposure works.

## 5. Exposure control (phase 2, first item)

Linux tracking frames run at 10410/449 regardless of the scene: overexposed indoors in daylight (selfcal saw
this), black in a dark room (the 00:14 clip). Long exposure also blurs during head turns, which is the worst case
for optical flow. Windows sends `58 19 51 00 01 32 | e0 e1 e2 e3 | g0 g1 g2 g3 | led_e led_g` (u16 LE) at
12.5 Hz, e.g. exposures `0c47 0c47 0857 0857`, gains `000d x4`, LED `0028 003e`. The LED pair matches the 40/62
that our LED frames carry, so the field layout is very likely right. Plan:

1. Rust `pimax-cam` AE loop: mean/percentile of the tracking tiles (ignore saturated LED blobs), target mid-grey
   with a cap on exposure (motion blur) before raising gain. Send `0x51` through libpimax at <= 12.5 Hz; the
   frame metadata echoes exposure/gain, so the loop can be closed and checked offline.
2. Keep all four exposures equal per pair (stereo matching), LED pair unchanged.
3. Test offline first: record with AE, check that `omarchyvr-slam timing` shows changing values and the images look
   right, then replay through Basalt.

This sends a command Windows sends all the time, so the risk is low, but it is a headset command: it goes in only
after review and with an off switch (`PIMAX_CAM_AE=0`).

## 6. Time synchronisation

* Same clock for camera and IMU: done (section 3). Residual camera-IMU offset: the stamp may mark the start of
  exposure, the end, or the start of readout. With exposure fixed at 10410 the midpoint could be several ms
  away. The factory `delta` = 1.638 ms is a hint (Pimax's own estimate, convention unknown).
* Knobs: `PIMAX_SLAM_CAM_OFFSET_US` shifts the frame times live. `CAM_OFFSET_NS=... run_slam.sh replay DIR` (=
  Basalt `cam_time_offset_ns`) does the same on a recording, so the offset can be swept offline (-5..+5 ms in
  1 ms steps; the best run has the smallest loop drift / RPE). Basalt does not estimate time offset online.
* Once AE changes exposure per frame, the midpoint correction becomes `stamp + exposure/2` per frame. The
  metadata carries the exposure, so this goes in the Rust frame timing once the unit (rows or µs) is known.

## 7. Failure handling, prediction, recentering

* **Supervisor** (`pimax_cam::slam::Supervisor`, unit-tested, called from the camera thread after every frame set
  and from `get_tracked_pose`):
  * *acquire* after 12 consecutive consistent poses (about 0.5 s). Consistent means: speed <= 4 m/s between
    poses, and the tracker's relative rotation within 8 deg of the gyro's (3DoF) relative rotation over the same
    interval;
  * *lose* on a jump, a rotation disagreement, a non-finite value, or no new pose for 350 ms;
  * after 3 s lost, *ask for a tracker reset* (`t_slam_reset`), at most every 6 s.
* **Continuity.** On acquisition the tracker world is yaw-aligned so that the head lands on the held position
  with the current 3DoF heading. The first acquisition therefore starts at the origin, exactly where 3DoF
  reported it. On loss the last good head position is held and the 3DoF orientation gets a yaw fix so the view
  does not jump. The position flag stays VALID but not TRACKED, and the angular velocity stays valid for
  timewarp. Re-acquisition re-aligns to the held position, which also absorbs a tracker reset (new world).
* **Prediction.** `t_slam` default `SLAM_PRED_DEAD_RECKONING`: the last VIO pose plus the IMU samples after it,
  with velocities from the latest sample. The compositor asks for display time, ~20-35 ms ahead of the newest
  image (12 ms delivery + Basalt ~5-10 ms + display). Rotation stays IMU-fast; position error grows with the
  square of the prediction time. Tune `SLAM_PREDICTION_TYPE` and the filter (`SLAM_UI` sliders) with data.
* **Recentering.** Debug UI button "Recenter 6DoF" moves the head to the origin with the heading unchanged.
  OpenXR apps also have Monado's own reference-space recentre. No floor height yet: LOCAL_FLOOR uses Monado's
  default. Floor estimation comes with the room map (controllers on the floor, or plane fit).

## 8. Relocalisation and room memory (phase 3)

Basalt's VIT build starts a new world every session and has no map save/load. The plan from
`selfcal-roommap.md` §5.2 fits on top of the supervisor, which already owns a `world_from_slam` transform:

1. Rust room map per room (`~/.local/share/omarchyvr/rooms/<uuid>/`): keyframes (pose + binary descriptors from all
   four cameras), landmarks, gravity-aligned room frame, floor, boundary.
2. At session start (and after a long loss), place recognition plus a multi-camera PnP with gravity known. The
   result `room_from_slam` replaces the supervisor's yaw-only alignment, so the room stays put across sessions
   and the boundary and anchors keep working.
3. Same data feeds the camera self-calibration (refined rotations of the upper cameras).

Until then, every session starts at the origin facing the initial 3DoF heading (like today's 3DoF).

## 9. Test plan

**Offline (no headset):**
1. `omarchyvr-slam check <rec>`: rates (25 Hz / 1 kHz), gaps, monotonic stamps, cam0/cam1 sharing every timestamp,
   all frames inside the IMU span, |accel| ~ 9.81. Unit-tested on synthetic data.
2. `run_slam.sh replay <rec>`: Basalt through `monado-cli slambatch` in real time (`EUROC_MAX_SPEED=false`; at
   max speed the player outruns the tracker and Basalt drops everything). Output `tracking.csv`, `timing.csv`.
3. Metrics (`omarchyvr-slam`):
   * `loop`: end-start distance for a run that ends where it started (headset put back on the same marked
     spot). Target < 1 % of the path, < 5 cm for a 5 m loop;
   * `ate EST REF`: rigid (Horn) alignment, ATE RMSE/median/max, RPE over 1 s. For runs with a known reference
     (e.g. two trackers, 2 vs 4 cameras, offsets). The Windows PVR log (`pimax-poses.csv`) loads with
     `--ref-format pimax-windows`, but it was never recorded together with Linux camera data, so it only
     serves future paired runs;
   * known distance: slide the headset 1.00 m along a table edge; the tracker should report 1.00 +- 0.02 m.
4. Sweeps on the same recording: camera offset (`CAM_OFFSET_NS`), IMU noise (default vs measured), 2 vs 4
   cameras, detection grid. Same data each time, so differences are real.
5. Already done: pipeline smoke test (`omarchyvr-slam smoke-euroc` from the dark 00:14 clip with a synthetic still
   IMU): Basalt loads the calibration, initialises and writes 365 poses for 367 frames. The poses drift away
   because the images are sensor noise and the IMU is fake, which is expected. This proves the plumbing only.

**Live (user), in order:**
1. *Recording, 90 s, lit room* (`run_slam.sh record`): 5 s still on the desk, then hold or wear it, look
   around slowly, walk a 3-5 m loop, near and far views, back to the **marked start spot**, 5 s still. Then
   `run_slam.sh replay <dir>` and `omarchyvr-slam imu-noise <dir>`. This gives accuracy numbers, the IMU noise, and
   the dataset for every later sweep. A second recording with fast head turns gives the robustness case.
2. *Live headless* (`run_slam.sh live 60`): `posecli` prints the position. Move the headset by hand 30 cm left,
   right, up and down; `p=` must follow with the `6dof` flag, then put it back. The log shows `6DoF: tracking`,
   and `lost (...)` with a reason when it fails.
3. *Worn, in a scene with near geometry* (a new test client or a real app via xrizer; `xr_room` is a sky sphere
   at infinity, so translation does not show there): lean and step, check world-locking, cover the cameras to
   force a loss, and check that the view holds without a jump and re-acquires.
4. After AE (phase 2): repeat 1 in bright and dim light.

## 10. Phase 1: what exists, how to run it

| Piece | Where |
|---|---|
| Basalt (VIT 2.0.1, matches Monado) | `~/src/basalt` (df6e970), `~/.local/lib/libbasalt.so`; build: `-DBASALT_BUILD_SHARED_LIBRARY_ONLY=ON -DCMAKE_POLICY_VERSION_MINIMUM=3.5 -DSKIP_PERFORMANCE_COMPARISON=ON` |
| Driver glue (C, thin) | `forks/monado` branch `slam`: `pimax_camera.c/.h`, `pimax_hmd.c`; `t_tracker_slam.cpp/t_tracking.h` (latest pose, reset) |
| Build | `~/.local/lib/omarchyvr/monado` (`-DPIMAX_CAM=ON`, Mercury on, `PIMAX_RUST_TARGET_DIR=$CARGO_TARGET_DIR/release`) |
| Rust: calibration export, IMU model, supervisor + C ABI | `rust/pimax-cam/src/slam.rs`, `capi_slam.rs`, `include/pimax_cam.h` |
| Rust tools | `rust/omarchyvr-slam` (`export`, `check`, `imu-noise`, `timing`, `ate`, `loop`, `smoke-euroc`) |
| Launcher | `analysis/tools/run_slam.sh record / live / replay` |

Env switches (all default off): `PIMAX_SLAM=1`, `PIMAX_SLAM_RECORD=<dir>|1`, `PIMAX_SLAM_CAMS`,
`PIMAX_SLAM_CAM_OFFSET_US`, `PIMAX_SLAM_IMU`, plus Monado's `VIT_SYSTEM_LIBRARY_PATH`, `SLAM_CONFIG`, `SLAM_UI`,
`SLAM_PREDICTION_TYPE`. Recording layout: `<dir>/euroc_<datetime>/mav0/{cam0,cam1,imu0}` +
`<dir>/pimax-basalt-calib.json` + `pimax-slam-info.json`. The main `~/src/monado` build is untouched (`PIMAX_CAM=OFF`
there, and `PIMAX_SLAM` would only print a warning).

Not done in phase 1: `basalt_vio`/GUI tools (they need Boost headers: `sudo pacman -S boost` would allow it, not
required), auto-exposure, relocalisation, a translation-showing test client, a merge into `omarchyvr`.

## 11. Next steps

1. User: lit 90 s recording -> replay -> numbers (section 9, live 1). Then live headless test (live 2).
2. Auto-exposure in Rust (section 5); camera offset sweep; 2 vs 4 cameras.
3. Merge `slam` into `omarchyvr` once live 2 passes (default stays off), then make 6DoF the default when worn tests
   pass.
4. Room map / relocalisation layer (section 8).

## 12. Phase 2 (2026-10-14): world locking, AE, room memory

**Why content moves today (plain).** Tracking is 3DoF: the IMU measures rotation only. When you lean or step, the
headset does not know it moved, so everything moves with you; and with no visual reference the heading (yaw)
slowly drifts, so the screen also creeps sideways. This is a software gap, not a hardware limit: Windows tracks
6DoF with the same four cameras. What does matter physically: cameras at 25 Hz per camera (the IMU bridges),
exposure (fixed 10410 us on Linux = blur + over/under-exposure; AE fixes it), light (a dark room gives no image:
the dark test clip has mean 6/255 even at full power-on exposure), texture (bare walls give no features; Windows'
own tracker logged "Not enough matched features" and reset twice in its 2-minute walk), and the tracking pair
diverging by 54 deg (little stereo overlap).

**Built (all default off, nothing sent to the headset):**
* AE: `analysis/slam-exposure.md` (frame, ranges, evidence; needs approval to send). Frames are stamped at
  mid-exposure (`PIMAX_SLAM_EXPOSURE_MID=1`).
* Head pose output in Rust (`pimax-cam/src/headpose.rs`, used by the driver for `PIMAX_SLAM=1`): IMU-rate
  rotation and position prediction from the newest tracker pose (gyro bias learned from the tracker), every
  correction blended (decay 150/100 ms, rate limit 0.1 m/s + half the head speed), and room re-snap after a loss
  in the same tracker world. On the real Windows head motion (`omarchyvr-slam simulate`): frame-to-frame error
  2.66 -> 0.38 mm RMS (max 13.8 -> 1.5 mm), absolute error p95 5.6 -> 3.0 mm; a 1.5 s dropout restores the room;
  a 3 s one triggers a tracker reset and needs room memory.
* Recenter that keeps the room: use Monado's own `monado-ctl -c` (moves LOCAL only, STAGE/room stays). The
  driver's debug button still re-anchors the whole room (debug only).
* Room memory (`rust/omarchyvr-room`, offline prototype, no driver hook yet): Basalt has no map/loop closure/reloc
  (checked in its VIT build), ORB-SLAM3 is GPLv3 (benchmark only), so our own Rust layer: ORB on a common upright
  virtual pinhole view, sub-pixel stereo, keyframes in room coordinates (`~/.local/share/omarchyvr/rooms/<name>/`),
  gravity-aware 4-DoF PnP relocalisation, anchors in room coordinates. Synthetic room through the real
  calibration: second session relocalises on its 2nd frame, 0.8 mm / 0.01 deg; a different room is rejected.
* Validation: `omarchyvr-slam stats` (still/HF noise, drift per metre, `--vs` Windows log), `simulate`, `shift` +
  `run_slam.sh sweep` (Basalt ignores `cam_time_offset_ns`, so offsets shift the data). Windows log reference:
  664 Hz, still noise 1.3 mm, HF 0.45 mm p50, two tracking gaps (0.6 s, 3.0 s) = its two SLAM resets.
* 4 cameras: Basalt takes N cameras (cam0 is the hub, other cameras also detect in their non-overlapping areas;
  Rift S runs 5); `PIMAX_SLAM_CAMS=0,1,2,3` is wired. Effort: an offline A/B on one recording; the upper cameras'
  3-15 px calibration disagreement must be fixed first (selfcal).

**Live tests, in order:** (a) `run_slam.sh record 90` in a lit room: 5 s still on a marked spot, 3-5 m loop,
back to the spot, 5 s still. (b) `run_slam.sh replay DIR` (loop drift, stats vs Windows), `run_slam.sh sweep DIR`
(best offset ~0 confirms mid-exposure), `omarchyvr-slam imu-noise DIR`; two recordings -> `omarchyvr-room build` +
`omarchyvr-room reloc` (time to first fix, alignment spread). (c) `run_slam.sh live` then worn with a static object
(WayVR panel at ~1 m, or a near-geometry scene): leaning 20 cm and stepping must leave the panel fixed on the
desk (no swimming, at most a slow mm slide after a correction); covering the cameras 1-2 s holds the view, and
on uncovering the panel slides back to its spot within ~1 s. After AE approval: repeat (a) with `PIMAX_CAM_AE=1`.

**Next:** AE approval + recording; driver hook for `omarchyvr-room` (worker thread at ~5 Hz, load/save per room,
`Supervisor` alignment from relocalisation); descriptors on Basalt's own landmarks (VIT pose-features
extension) for better map depth.
