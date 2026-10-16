# Tracking-camera auto-exposure (AE): evidence, frame, controller (2026-10-14)

Status: controller implemented in Rust (`rust/pimax-cam/src/ae.rs`, C ABI `capi_ae.rs`), wired into the Monado
driver (`~/src/monado-slam`, branch `slam`) behind `PIMAX_CAM_AE`, **default off. Never sent to the headset yet:
sending needs the user's approval** (the frame is derived from Windows captures).

## 1. Why
Linux leaves the tracking cameras at the power-on 10410/449: overexposed in a lit room, black in a dark one, and
~10 ms of exposure blurs during head turns. Windows' Pimax tracker pushes exposure/gain at 12.5 Hz.

## 2. No UVC alternative (checked read-only, camera idle)
`v4l2-ctl --list-ctrls`: no controls. Camera Terminal bmControls = 0, Processing Unit bmControls = 0. One extension
unit {c2b1ccad-abf6-48b8-8e37-32d4f3a3feec} with 5 selectors (2, 3, 10, 11, 12), which Windows never uses (it only
probes them at enumeration). So the HID command below is the only known path.

## 3. The frame (HID headset 34a4:0044, interrupt OUT EP 0x01, 64 bytes, zero padded)
```
58 19 51 00 | 01 32 | e0 e1 e2 e3 | g0 g1 g2 g3 | 28 00 3e 00        u16 little endian
reply (IN EP 0x81): 4c 04 51 00 01 07
example (Windows, slam capture #20): 58 19 51 00 01 32 67 07 67 07 a6 0c a6 0c 0d 00 0d 00 0d 00 0d 00 28 00 3e 00
```
Through libpimax: `pimax_send(h, 0x51, 0x00, payload, 22)`; payload = `pimax_cam::ae::payload()`.

## 4. Evidence (`analysis/data/ae-win-*.csv`, `analysis/ae-win-report.txt`; M = measured, I = inferred)
| Item | Value |
|---|---|
| Rate | 1876 commands / 149.9 s, median interval 80.000 ms (12.5 Hz), one per 4 camera frames, sent ~1.8 ms after a tracking frame arrives [M] |
| Replies | all 1876 = `4c 04 51 00 01 07` [M] |
| Constant parts | `01 32` always (0x32 = 50: frame-sync rate request [I]); LED pair 40/62 always [M] |
| Pairs | e0=e1, e2=e3, g0=g1, g2=g3 in every command: lower pair (cams 0,1) and upper pair (2,3) [M] |
| Exposure | lower 1682..4114 (median 3051), upper 1140..4167 (median 3238) [M] |
| Gain | lower 13..29, upper 13..215; above 13 only at exposure >= ~3.6k (ceiling ~4.1k) [M] |
| Steps | geometric exposure ladder, ratio 1.0302 (34/33); typically 1 step (3 %) per command, max 3; 58 % of commands change nothing [M] |
| Latency | new values in the frame metadata from the 2nd tracking frame after the send (46-62 ms); metadata always equals a sent command [M] |
| Target | trimmed mean ~62/255 for both pairs [I, low confidence: only 25-row pixel windows survive in the pcap] |
| Unit | exposure = microseconds: each tile's first image row = 10 + e/38.8 (= one line of a 516-line 20 ms frame) [I] |
| Frame time | the metadata stamp marks the exposure START; Pimax's factory `delta` 1.638 ms = half its typical 3.3 ms exposure [I] |
| Cold start | before the first command: 3500/60 at 60 fps; the first command re-inits the cameras (336 ms frame hole, 60 -> 50 fps) [M] |

## 5. Controller (Rust)
Histogram of each pair (both tiles, 4-px grid inside the fisheye circle, top 2 % dropped), error ln(64/mean) with
±8 % deadband, damped step relative to the frame's own metadata exposure*gain (no wind-up with the 2-frame
latency), at most 3 ladder steps per command (2x when the image is mostly saturated/black), exposure first up to
4108 then Windows' gain codes, values snapped to Windows' ladder, never a step against the error (a room too dark
even for the power-on setting keeps it). Sent every 80 ms like Windows. Simulated sensor: settles from the
power-on default in 0.7-1.5 s (Windows' cold start: ~1.1 s). Dry run on the existing (dark) recordings: keeps
10410/449 (correct: nothing brighter is allowed).

Knobs: `PIMAX_CAM_AE=0|dry|1`, `PIMAX_CAM_AE_TARGET`, `_EXP_MIN` (100), `_EXP_MAX` (4108), `_GAIN_MIN/_MAX` (13/215),
`_PERIOD_MS`, `_K`, `_DEADBAND`, `_UPPER`, `_QUANTIZE`. Offline: `pimax-slam ae REC.pxcam`.

Frame time: the driver now stamps tracker/recorder frames at mid-exposure (`PIMAX_SLAM_EXPOSURE_MID=1`, default;
+5.2 ms at 10410). The offset sweep on the first lit recording confirms or rejects this (best offset ~0 = right).

## 6. What needs approval, and the first test
Approve sending `58 19 51 00 01 32 ...` with values only from the ranges above. First live run: `PIMAX_CAM_AE=dry`
(log only), then `PIMAX_CAM_AE=1` with `PIMAX_SLAM_RECORD=1`; expect one ~0.3 s camera gap at the first command,
then `pimax-slam timing` on a `.pxcam` / the 5 s stats line should show exposure moving into 1.1k-4.1k.

## Live walk 2026-10-14 15:22

First live 6DoF walk (`run_slam_view.sh 90`, `PIMAX_CAM_AE=0`), recording `20261014-152215-live` (2350 frame sets, 94046 IMU samples, no gaps).
User stood ~5 s, walked slowly, ended at the start.

- Coverage: 899 of 899 pose lines are `6dof` (100 %). Tracking acquired once (acquisition 1), 0 LOST, 0 snapped, 2205 blended.
- Position range: x 0.0..6.84, y 0.0..1.53, z -3.41..0.0 m. Path length 28.7 m. Closure error: end (6.28, 0.39, -1.72) vs start = 6.52 m (22 % of path).
- Jumps over 10 cm between consecutive 0.1 s lines: 24, in two bursts (t=9.9..11.3 s, up to 0.17 m; t=84.0..84.8 s, up to 0.19 m), i.e. fast motion rather than snaps. The pose is already 2.2 m from the start at t=5 s (user standing still) and 6.1 m at t=10 s: the drift happens immediately at start.
- Gyro bias converged to about (-0.15, 0.80, -0.38) deg/s; blend correction small (3.9 mm / 0.01 deg, max 77.5 mm / 1.20 deg).
- Passthrough/broker: broker 50.0 fps steady (5286 frames, 0 notices dropped, 5 source timeouts early and none after); camera images 24.4-25.1/s, capture->display ~35/42/49 ms (p50/p90/p99-ish), no errors.
- Camera brightness (cam0, 8-bit, 5 samples): mean 85, 162, 159, 116, 110; saturated (>=250) 1.2 %, 32 %, 23 %, 1.3 %, 5.6 %. Overexposed in the bright parts of the run (fixed exposure, AE off).
- Offline replay (`run_slam.sh replay`): end-start distance 6.98 m over a 29.66 m path (23.5 % loop drift), end-start rotation 6.67 deg. Matches the live result, so the live path is the same as the offline one and the error is in the SLAM, not in the live plumbing.
- Verdict: 6DoF tracks continuously and without losses, but the position is wrong (metres of drift while standing, 23 % loop drift). Likely cause: exposure saturation plus start-up scale/initialization drift. Next fix: enable the AE/exposure control (this document) to avoid saturation, and check the start-up phase (stationary 5 s should give ~0 m: IMU-to-camera extrinsics/time offset, accel units, stereo baseline scale).
