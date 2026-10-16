# Camera hand tracking (Mercury): offline results

Date: 2026-10-15. Recordings only: no headset access, no live session touched. Machine: i9-14900KF (32 threads).
Monado `hands-mercury` b6e46b1 (on desk cfad375), `~/.local/lib/omarchyvr/monado`, ONNX Runtime 1.20.1 CPU. Models:
`grayscale_detection_160x160.onnx` + `grayscale_keypoint_jan18.onnx` (hand-tracking-models 37a8a81, **no licence:
local use only**). How to use: `docs/HANDS.md`.

## 1. Rate in the service (rate fix: the next pair is accepted after 80 % of the period)

`analysis/tools/run_hands_offline.sh` (null compositor, private runtime dir, `PIMAX_NO_DEVICE=1`, pimax USB driver
disabled, `PIMAX_CAM_SOURCE=<.pxcam>` replayed in real time, service at nice 10, Mercury thread nice 10).
`hands (running)` lines are 5 s windows. Target: 25 pairs/s, which is every tracking-exposure pair of the 50 fps
camera.

| recording | pairs/s processed (steady windows) | offered | dropped per window | mean / max ms per pair | service CPU (whole process) |
|---|---|---|---|---|---|
| `~/ctrl-test.pxcam` (60 s) | 24.7-25.1 (the first window is 20.1 while the models warm up) | 24.9-25.1 | 0-1 | 1.6-1.9 / 3.5 | 29.6 % of one core: replay decode (`pimax-camera`) 21 %, `pimax-hands` threads about 8 % |
| `~/src/cam-data/clip-20261014-001422.pxcam` (60 s) | 22.4-25.1, following the frames the clip delivers (it loses frames itself: 45-50 fps) | 22.5-25.1 | 0-6 | 2.4-2.8 / 6.7 | 16.8 %: `pimax-hands` about 10 % |
| ctrl-test + `xr-pinchtest`, `PIMAX_HT_ROLES=hands` (30 s) | 24.9-25.1 | 24.9-25.1 | 0 | 2.8-3.1 / 15.9 | 42 % (replay 26.5 %, hands about 12 %). **xr-pinchtest PASS**: `hand_interaction_ext` binds to both camera hands (no hands in view, so the value stays 0) |
| ctrl-test, models missing (`NO_MODELS=1`) | 0 (hands off) | - | - | - | `hands: Mercury models not usable ... VR continues without camera hands`; the service stays up |

The replay decode cost (`pimax-camera`, 21-27 %) belongs to the `.pxcam` replay. A live session reads the broker.
Camera frames lost in the service: 0 for ctrl-test. All the losses in the clip come from the recording.

## 2. Per-pair cost and detection (`pimax_ht_bench`, nice 19, every tracking pair, synchronous Mercury)

| recording | pairs | wall ms mean / p95 | CPU ms mean / p95 | % of one core at 25 Hz | detection any / both | wrist distance p50 |
|---|---|---|---|---|---|---|
| ctrl-test.pxcam | 1492 | 2.73 / 4.57 | 6.25 / 9.10 | 15.6 | 0 / 0 | - |
| clip-20261014-001422.pxcam | 515 (all dark) | 2.62 / 4.49 | 6.04 / 9.01 | 15.1 | 0 / 0 | - |
| evening-20261013-1959, fixture8 | 3, 4 | 2.6-2.9 | 7.3-7.5 | 18-19 | 0 | - |
| EuRoC 20261014-150658 | 6 | 3.19 | 7.72 | 19.3 | 0 | - |
| EuRoC 20261014-151524 | 1179 | 2.89 / 4.72 | 6.49 / 9.54 | 16.2 | 0.3 % / 0 | 0.63 m (n 3, probably false) |
| EuRoC 20261014-152216 | 1175 | 2.31 / 4.39 | 5.52 / 8.84 | 13.8 | 0 / 0 | - |
| EuRoC 20261014-162452 | 1275 (122 dark) | 1.88 / 3.38 | 4.33 / 7.37 | 10.8 | 0 / 0 | - |
| EuRoC 20261014-163703 | 715 | 1.46 / 2.80 | 3.23 / 4.67 | 8.1 | 0 / 0 | - |
| **EuRoC 20261014-164421** | 771 | 1.98 / 4.32 | 4.69 / 11.21 | 11.7 | **11.4 % / 7.0 %** | **0.31 m** (0.28-0.37, n 142) |
| EuRoC 20261014-164606 | 901 | 2.96 / 5.00 | 6.63 / 10.56 | 16.6 | 1.1 % / 0 | 0.45 m (n 10) |

Pairs with a hand vs. without (from the per-pair TSV):

| recording | with a hand: CPU / wall ms | two hands: CPU ms | without: CPU / wall ms |
|---|---|---|---|
| 164421 | 11.30 / 4.08 (n 88) | 11.75 (n 54) | 3.83 / 1.72 |
| 164606 | 13.47 / 7.20 (n 10) | - | 6.55 / 2.91 |
| 151524 | 13.98 / 7.72 (n 3) | - | 6.47 / 2.87 |

**Summary:**
* No hand in view: 4-6.5 ms CPU per pair, which is **10-16 % of one core** at 25 Hz.
* Hands in view: 11-14 ms CPU per pair, which is **28-35 % of one core** at 25 Hz. A second hand costs almost nothing.
* Wall time per pair: 1.5-3 ms without a hand, 4-8 ms with one. This is far below the 40 ms period, so the 50 % duty
  limit never triggers.
* The earlier "42 % with one hand" figure came from one 1.5 s episode measured at nice 19 while a desk session was
  running. These numbers replace it.

## 3. Detection: what is known

* None of the recordings shows lit bare hands held on purpose. ctrl-test holds controllers. The SLAM recordings are
  head motion. 164421 has a real hand episode at arm's length (0.28-0.37 m, both hands in 7 % of pairs), which is
  consistent with someone reaching to the desk.
* Only the calibrated orientation (cams 0,1, roll 180/0) gave detections. Swapped order and other rolls gave 0
  (previous session). Every tile has its own start row from the frame metadata, and all 480 rows arrive, so the old
  466-vs-480 question is closed.
* **Open:** the detection rate on bare, lit hands. This needs a new recording: `docs/HANDS.md`, "Live test", step 4.

## 3b. Live Work session recording `~/src/cam-data/hands-live-1458.pxcam` (2026-10-15, 0 % live)

Live: `desk --fps 72 --no-eye --6dof --ctrl-track --hands --voice --anchor` said "left active 0 %, right 0 %" at
25 pairs/s. Suspected: Mercury gets the dark controller-LED frames. **Not the cause.**

* What Mercury gets: `pimax_camera.c` `capture_thread` skips every frame whose kind is not `TRACKING` before it
  builds the Mercury pair, so the LED frames reach only the debug sinks. The live log agrees: camera 50.04 fps,
  tracking 25, "Mercury pairs pushed 23329" of 46658 frames (exactly half), Mercury 25 pairs/s.
* The recording: 2002 frames = 1001 tracking (exposure 3.6-4.0 ms, gain 17-29, mean grey 53/67 for cams 0/1, never
  dark) + 1001 LED (exposure 40 us, gain 62; 975 of 1001 pairs dark).
* `pimax_ht_bench --kind tracking|led|any` (new option): **0 % / 0 % / 0 %** detection.
* The images explain it: **no hand is in front of the cameras at any time.** The headset lies still on the bed
  (cam 0 is half covered by the bedspread). In the first ~8 s a person gets up from the bed next to it; then they sit
  at the desk about 2 m away with their back to the headset. From 10 s to 40 s the grey level of both cameras
  changes by less than 1 (a static scene).
* Orientation and tile order on this live data: tiles 0 and 1 see the same desk and room (stereo overlap), tile 0 is
  upside down and tile 1 upright, which matches the calibrated 180/0. Tiles 2/3 look elsewhere (ceiling, window).
  This recording cannot confirm a detection, because it has no hands.
* Exposure: the tracking frames are well exposed for hands (mean grey 53-67, only the window saturates). No exposure
  change is needed. Nothing sends HID bytes.
* **Next:** repeat the guided hand test with the headset **on the head**, bare hands 25-50 cm in front, in a lit room.

## 4. Build state

* `ninja -C build-ht` reports no work to do on b6e46b1, so it is clean. The service links
  `~/src/onnxruntime-1.20.1/lib/libonnxruntime.so.1` through RUNPATH. This is an absolute path, so it still works
  after `setcap` (secure-execution mode ignores only `$ORIGIN` and `LD_LIBRARY_PATH`).
* `getcap` on the build-ht service is empty: install step 2 in `docs/HANDS.md` is needed before a live start.
