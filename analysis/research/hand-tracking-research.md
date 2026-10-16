# Controller-free hand tracking for the Pimax Dream Air on Linux (Monado)

Date: 2026-10-15. Read-only research. Nothing built, committed or run. Verified = read in local source/files or a fetched page this session. UNVERIFIED = search snippet only, or not checked.

## 1. Our hardware (local notes)

| Item | Value | Source |
|---|---|---|
| Tracking cameras | 4 x OV7251-class mono greyscale (8-bit), arranged as 4 tiles of 640 px in one UVC frame | `analysis/camera-frames.md` |
| Frame | `/dev/video0`, UVC, 2560x832 8-bit grey (advertised as YUY2 1280x832). Image rows 24..489 (466 rows); rows 0-7 metadata | `analysis/camera-frames.md` |
| Rate | 50.03 fps (100 MHz PTS clock). Frame counter consecutive, no drops in 3 s | `analysis/camera-frames.md` |
| Timestamps | Metadata u32 us timestamp uses the same MCU clock as the 500 Hz IMU (frame ts - IMU = -11.8 ms mean, std 0.7 ms) | `analysis/camera-frames.md`, `analysis/imu-camera.md` |
| Colour / IR | Greyscale. No IR filter or NIR illuminator mentioned in our notes | `analysis/camera-frames.md` |
| Calibration | `analysis/headset-calibration.xml` (also `headset-calibration-linux.xml`). Four cameras `trackingA` (id 0), `trackingB` (1), `Pi-controllerA` (2), `Pi-controllerB` (3); model `FISHEYE_4_PARAMETERS`; size 640x480; Rig extrinsics per camera | same |
| Stereo baseline | trackingA to trackingB: Rig translation (-0.0997, 0.0071, -0.0497) m, length about **0.112 m**. Pi-controllerA is about 0.04 m from A and is not a good pair. Reference frame of Rig translations is UNVERIFIED | calibration XML (length computed by hand) |
| Tile order | Cam 0..3 left to right assumed, UNVERIFIED | `analysis/camera-frames.md` |
| Frame transport | `pimax-cam broker`: one V4L2 reader, shared memfd ring (default 8 slots, 2560x832 raw) plus SOCK_SEQPACKET notices. Any number of processes read the same frames | `analysis/camera-broker.md` |

**Open mismatches:** the calibration says 640x480 per camera, but only 466 rows carry image (rows 24..489). Either the crop is 7..486 or the calibration size is wrong. This must be settled before Mercury gets a `t_stereo_camera_calibration`. Also the calibration has `TimeAlignment delta="0"` and a separate `SFConfig delta=0.001638`.

## 2. Monado in our tree (`~/src/monado-eye`, branch `desk`, HEAD 5aa6c98)

**Our build does not have hand tracking.** From `build/CMakeCache.txt`:
- `ONNXRuntime_LIBRARY` and `ONNXRuntime_INCLUDE_DIR` are `NOTFOUND`, so `XRT_HAVE_ONNXRUNTIME` cannot be on.
- `XRT_MODULE_MERCURY_HANDTRACKING` and `XRT_BUILD_DRIVER_HANDTRACKING` do not appear in the cache at all. Both depend on ONNX Runtime and OpenCV (`CMakeLists.txt:289`, `:361`).
- OpenCV is found (`/usr/lib/cmake/opencv5`, `XRT_HAVE_OPENCV` on). Only ONNX Runtime is missing.
- `XRT_BUILD_DRIVER_PIMAX` and `PIMAX_CAM` are ON, so the Pimax driver is built but has no Mercury.

**The Pimax driver already has a Mercury path, but it is compiled out.** `src/xrt/drivers/pimax/pimax_camera.c`:
- `setup_mercury()` (line ~1090) is wrapped in `#ifdef XRT_BUILD_DRIVER_HANDTRACKING`. Without it, `PIMAX_HANDS=mercury` logs an error and returns false.
- Wiring: `PIMAX_HT_CAMS` picks two camera ids (default unverified) -> `make_stereo()` -> `ht_device_create(&xfctx, stereo, create_info, &hand_sinks, &ht)` -> `u_sink_split_create` per view (to Mercury and to a SLAM stub) -> `multi_create_tracking_override(XRT_TRACKING_OVERRIDE_ATTACHED, ht, hmd, XRT_INPUT_GENERIC_HEAD_POSE, &offset)`.
- So hands are attached to the **head pose**, not to the SLAM world pose. This is fine for a head-locked test, but world-locked hands need the SLAM pose.
- The file header describes the same graph: `view 0 / view 1 -> u_sink_split -> Mercury view sink (t_hand_tracking_async)`.

**Mercury files (verified):**
- `src/xrt/tracking/hand/mercury/` (hg_sync.cpp, hg_model.cpp, hg_image_distorter.cpp, kine_lm/, hg_interface.h). Licence header BSL-1.0 (verified in `hg_interface.h`).
- Public API: `t_hand_tracking_sync_mercury_create(calib, create_info, models_folder)` in `hg_interface.h`.
- Models loaded in `hg_model.cpp`: `grayscale_detection_160x160.onnx` (line 298) and `grayscale_keypoint_jan18` (line 437). **The greyscale variants exist, so we need no colour model.**
- Mercury uses ONNX Runtime through `src/xrt/auxiliary/onnx/` (`aux_onnx`, `onnx_wrapper.cpp`). Our build lacks it.
- Frames are L8 in the visualiser (`hg_sync.cpp:769`).

**Other in-tree hand sources:** `src/xrt/drivers/ht/` (ht_driver.c, the generic camera hand driver, `ht_device_create`), `src/xrt/auxiliary/util/u_hand_simulation.c` (controller-emulated hands, `ht_ctrl_emu`), `src/xrt/drivers/ultraleap_v2`, `ultraleap_v5`, `north_star`, `rift`, `rift_s`, `wmr`, `vive`, `survive`. The Rift S driver uses `u_sink_split` to feed the same camera frames to SLAM and hand tracking (`rift_s_camera.c`, `in_sink` at line 57). That is the pattern we want.

**Models download:** `scripts/get-ht-models.sh` (verified). It clones `https://gitlab.freedesktop.org/monado/utilities/hand-tracking-models` into `~/.local/share/monado` and runs `git lfs pull`. Needs `git-lfs`. There is also `update-ht-models.sh`.

**Not verified:** whether the model weights are under a licence (the repo page is behind an Anubis bot check), and their exact sizes.

## 3. Web research

### 3.1 Monado Mercury: current state
- Monado hand tracking page, https://monado.freedesktop.org/handtracking (fetched, verified):
  - Supported: Valve Index; most Luxonis stereo cameras (via North Star builder or the monado-gui "Hand-Tracking Demo"); Windows Mixed Reality and Rift S "work, with degraded quality due to autoexposure issues".
  - "Basically any calibrated stereo camera should work." RealSense or Vive Pro would need "some plumbing".
  - Quality "generally good on Index and Luxonis cameras, but it's still in development".
  - Lighting: needs "plenty of light"; front lighting helps.
  - Build: OpenCV, ONNX Runtime (CPU build, not GPU/TensorRT; no official packages), optional depthai.
  - No latency or accuracy figures. Models and their repo are not named on that page.
  - Head tracking caveat: libsurvive head tracking "quite imperfect" on Index.
- Collabora announcement, 2023-02-24, https://www.collabora.com/news-and-blog/news-and-events/monados-mercury-hand-tracking-now-ready-for-use.html (search snippet): work began about Jan 2022. Usable for drawing, typing and UI. Stability and jitter work planned. A detailed follow-up post was promised; I did not find it.
- Development repo: https://gitlab.freedesktop.org/monado/utilities/hand-tracking-playground/mercury_train (listed in search; README and LICENSE not readable).
- Merge request 2228, https://gitlab.freedesktop.org/monado/monado/-/merge_requests/2228 (search snippet): Mercury plus Basalt VIO on a simulated North Star with an OAK-D Pro, with IMU and axis fixes. This is the closest prior art for Mercury plus SLAM in one process.
- Mono vs stereo: Mercury is multi-view triangulation. Docs say "stereo"; mono is **not** supported in-tree (UNVERIFIED for mono-only fallbacks).
- IR/greyscale: docs silent. Our source has `grayscale_*` models, so greyscale input is supported by design (verified in source).
- Latency and CPU cost: **no published figures found.** Monado says the models run on CPU.
- Licence: code BSL-1.0 (verified in source headers). Weights: **UNVERIFIED**.
- Known issues: autoexposure on WMR and Rift S (docs). Our Pimax driver already has an AE mode (`pimax_cam_ae`, default off).

### 3.2 Alternatives on Linux

| Option | Input | Licence (code / weights) | Output | Latency (cited) | Greyscale/IR fisheye | Runtime | Notes |
|---|---|---|---|---|---|---|---|
| Monado Mercury | 2 calibrated greyscale views | BSL-1.0 / UNVERIFIED | 26 joints (OpenXR) | no figure | models are greyscale (verified in source) | ONNX Runtime CPU | in-tree, our target |
| MediaPipe Hands / Hand Landmarker | RGB frame | Apache-2.0 code (verified via survey); weights UNVERIFIED | 21 landmarks per hand, plus world coords | Pixel 6: 17.12 ms CPU, 12.27 ms GPU (Google table, via guide https://developers.google.com/edge/mediapipe/solutions/vision/hand_landmarker/index) | not for IR; RGB-trained | TFLite; CPU/GPU | needs 21-to-26 joint mapping |
| Ultraleap Gemini / Leap | Leap Motion Controller (NIR stereo) | Proprietary; personal/eval use only (survey, https://docs.ultraleap.com) | 26 joints via OpenXR layer or Monado `ultraleap_v2`/`v5` driver | hardware | n/a (own sensor) | `leapd` service | Linux x64 listed; needs Leap hardware; not usable commercially without agreement |
| RTMPose (hand model in MMPose) | RGB | Apache-2.0 per a third-party HF page (UNVERIFIED for hand weights) | 2D keypoints (21 hand) | published numbers are for body models (ONNX i7 1 thread: ~90+ FPS; TensorRT GTX 1660 Ti: ~430+ FPS) | not tested on fisheye; no hand numbers | ONNX Runtime, TensorRT (via rtmlib) | 2D only; would need depth or triangulation |
| HaMeR | RGB, single image | MIT code; MANO body model separately licensed | MANO mesh + 3D joints | UNVERIFIED (ViT-H, likely not real-time CPU) | RGB | PyTorch | licence risk on MANO |
| WiLoR | RGB, single image | Models CC-BY-NC-ND (verified in survey) | MANO-based 3D | UNVERIFIED (has `--fast` mode) | RGB | PyTorch | non-commercial only |
| UmeTrack / HOT3D (Meta) | multi-view headset cameras | CC BY-NC 4.0 / CC BY-NC-SA hand data | datasets | n/a | n/a | n/a | research/training only |
| FrankMocap, MobRecon, HandOccNet, A2J | - | NOT RESEARCHED | - | - | - | - | not checked |

Local survey with the same entries: `analysis/research/handtracking-oss-survey.md`.

### 3.3 Pimax Dream Air on Windows
- The Dream Air SLAM edition has four onboard cameras, "over a million tracking points", and hand tracking. The store lists hand tracking as SLAM-only. The Lighthouse edition has no hand tracking. Sources: https://store.pimax.com/products/pimax-dream-air?country=BR, https://simracingcockpit.gg/pimax-dream-air-slam-review/, https://www.auganix.org/?p=17112 (search snippets and page text).
- Camera module or vendor: **not confirmed by any web source.** Our USB descriptor says `34a4:0014`, "eYs3d Technology", "PimaxSensor" (`analysis/research/dreamair-cameras.md`).
- Passthrough: black and white, low resolution (one spec table).
- Pimax's Windows hand-tracking path is in the closed PVR SDK: a 32-bone skeleton in the PVR API headers (`analysis/customheadsetopenvr.md`). No Ultraleap module is confirmed.
- Resolution, fps and IR/RGB are not in any web source. Our decode (greyscale 640x466, 50 fps) is the best data.

### 3.4 Prior art: custom cameras into Mercury
- Monado's own page says any calibrated stereo camera works (above). Mercury's device path is `ht_device_create(...)`, which takes `struct t_stereo_camera_calibration *` and `struct t_hand_tracking_create_info` (`cams_info` with `t_camera_extra_info`: camera orientation, boundary type), plus a `models_folder`.
- Rift S driver (`src/xrt/drivers/rift_s/rift_s_camera.c`): `xrt_frame_sink` receives raw frames and `u_sink_split` feeds Mercury and SLAM.
- MR 2228 (above): simulated North Star with OAK-D Pro, Mercury plus Basalt.
- Required inputs, from code: calibration (`t_stereo_camera_calibration`; fisheye 4-parameter model is supported by `t_camera_models.cpp`, which we read as a name only); `xrt_frame` L8 greyscale per view; monotonic timestamps on the same clock as the IMU (we have that, per `analysis/camera-frames.md`).
- Calibration format: Monado uses its own `t_stereo_camera_calibration` (from OpenCV-style intrinsics/extrinsics). Our Pimax XML needs converting. No prior art found for this exact XML.

### 3.5 GPU budget
- Monado documents CPU inference only. The GPU is not needed for Mercury (verified in docs).
- No published CPU ms-per-frame figure for Mercury (UNVERIFIED). MediaPipe full hand pipeline: 17 ms on a Pixel 6 CPU (Google table).
- At 72 Hz the frame budget is 13.9 ms. Mercury on a separate CPU thread at 50 Hz camera rate does not touch GPU. The risk is CPU contention with the compositor and Basalt, not GPU. Measure on our box before deciding (not done here).

## 4. Recommended path (5 lines)
1. Get ONNX Runtime (CPU build) installed and enable `XRT_MODULE_MERCURY_HANDTRACKING` and `XRT_BUILD_DRIVER_HANDTRACKING` in a **separate** build dir (not `build/`, which is live). Run `scripts/get-ht-models.sh`.
2. Settle the calibration: write a converter from `headset-calibration.xml` to Monado's `t_stereo_camera_calibration` for the pair trackingA (0) and trackingB (1), baseline about 0.112 m, and resolve the 466-vs-480 row mismatch.
3. Feed greyscale tiles 0 and 1 through the existing `setup_mercury()` path (`PIMAX_HANDS=mercury`, `PIMAX_HT_CAMS=0,1`), via `pimax-cam broker` as the frame source, and check the 26 joints with an `XR_EXT_hand_tracking` test app.
4. Measure CPU ms per frame and latency with the head pose (current wiring attaches hands to the head, not SLAM world pose). Then switch the attachment to SLAM once Basalt is live.
5. Decide later on a stronger model only if Mercury's accuracy fails on the greyscale fisheye (then MediaPipe-style RGB is not an option; no greyscale-trained alternative was found).

## 5. Biggest risks
- **Model licence and availability:** the weights' licence and download reliability are unverified (repo behind a bot check). Check before any product use.
- **No accuracy or latency data:** Mercury was tuned on Index and Luxonis (greyscale and RGB, different optics). Our fisheye lens, 466-row crop and 50 Hz frame rate are untested. Expect a lot of tuning.
- **Calibration mismatch and tile order:** 466 vs 480 rows, unverified Rig frame and tile order. A wrong baseline or order gives silently wrong 3D hands.
- **CPU contention:** Mercury on CPU may compete with the compositor and Basalt on the same box. Not measured.
- **Head-attached hands:** the current driver wiring ties hands to the head pose, so hands drift with head motion when SLAM is off or wrong.
- **Ultraleap is not a fallback for us:** it needs Leap hardware and personal-use licence only.
- **Build breakage:** our `build/` has no ONNX; `PIMAX_HANDS=mercury` today errors out.

## Sources
- https://monado.freedesktop.org/handtracking (fetched)
- https://gitlab.freedesktop.org/monado/utilities/hand-tracking-playground/mercury_train (search)
- https://www.collabora.com/news-and-blog/news-and-events/monados-mercury-hand-tracking-now-ready-for-use.html (search)
- https://gitlab.freedesktop.org/monado/monado/-/merge_requests/2228 (search)
- https://developers.google.com/edge/mediapipe/solutions/vision/hand_landmarker/index (search)
- https://github.com/google-ai-edge/mediapipe (licence per local survey, UNVERIFIED this session)
- https://github.com/geopavlakos/hamer, https://github.com/rolpotamias/WiLoR, https://github.com/facebookresearch/UmeTrack (per local survey `handtracking-oss-survey.md`)
- https://docs.ultraleap.com, https://monado.pages.freedesktop.org/monado/md_drivers_ultraleap_v2_readme.html (search)
- https://arxiv.org/pdf/2303.07399 (RTMPose)
- https://store.pimax.com/products/pimax-dream-air?country=BR, https://simracingcockpit.gg/pimax-dream-air-slam-review/, https://www.auganix.org/?p=17112 (Dream Air specs, search)
- Local: `analysis/handtracking.html`, `analysis/hands-desk.md`, `analysis/camera-frames.md`, `analysis/camera-broker.md`, `analysis/imu-camera.md`, `analysis/headset-calibration.xml`, `analysis/research/handtracking-oss-survey.md`, `analysis/research/dreamair-cameras.md`, `~/src/monado-eye/src/xrt/drivers/pimax/pimax_camera.c`, `~/src/monado-eye/src/xrt/tracking/hand/mercury/hg_model.cpp`, `~/src/monado-eye/scripts/get-ht-models.sh`, `~/src/monado-eye/build/CMakeCache.txt`
