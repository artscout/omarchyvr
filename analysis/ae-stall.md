# AE push stall: why IMU and cameras stopped after the first 0x51 (2026-10-14)

Run `~/.local/share/pimax/slam-recordings/20261014-150658-live` (`PIMAX_CAM_AE=1`, SLAM + EuRoC recording, Monado
`9dfbadc`). Analysed offline only: nothing sent to the device.

## Verdict
**The device did nothing wrong. A host-side deadlock in the Basalt tracker pipeline, triggered by the camera gap
that the first AE push causes (the same gap appears on Windows).** Fixed in `~/src/monado-slam` (branch `slam`,
commit `d3138d4`, local only).

## Evidence
1. **Bytes are right.** `ae::payload()` + `pimax_proto::build_command` + `to_write_buf` produce
   `00 | 58 19 51 00 01 32 e0 e1 e2 e3 g0 g1 g2 g3 28 00 3e 00 | zeros` (65-byte hidraw write = report id 0 + 64
   bytes), on IF0 through the same `Session::write_ctl` path as the 213 ms keepalive (interrupt OUT EP 0x01, not a
   SET_FEATURE). Windows cold1 #0: `58 19 51 00 01 32 98 08 98 08 98 08 98 08 0d 00 0d 00 0d 00 0d 00 28 00 3e 00`,
   64 bytes, bytes 26..63 zero, same EP. Test `payload_matches_the_windows_frame` (ae.rs) covers it. Windows sends no
   extra command around the first 0x51 (cold1 OUT list 9.5..14 s: only keepalives `58 04 39 00 5a`, `0a`, `56`,
   `2c`; no UVC re-commit on the camera).
2. **The headset reacted exactly as on Windows.** EuRoC cam0 (`euroc_20261014150658/mav0/cam0/data.csv`): frame
   8674.699 s, then a **512 ms hole**, then frames every 40.0 ms (25 Hz tracking = 50 fps camera) from 8675.211.
   Windows cold1: first 0x51 -> sensor re-init, 336 ms hole + stale frames, fresh frames 0.53 s later, 60 -> 50 fps.
3. **IMU stopped 0.32 s into the hole**: imu0 last sample 8675.023 s = 8674.699 + ~0.32 s, i.e. ~300 samples at
   1 kHz after the last frame the tracker consumed. No `AE: exposure push failed` warning, no kernel USB/uvc event
   (`journalctl -k` 15:06-15:09), no re-enumeration.
4. **Camera thread stopped 11 frames after the hole**: last frame 8675.611 (frames 8675.211..8675.611 = 11). The
   camera thread prints a stats line every 5 s even on timeouts (`update_stats` runs on every 200 ms
   `pimax_cam_next` return); the log has **none** in 60 s, so that thread was blocked, not starved.
5. Poses stay identity for all 60 s: the gyro calibration (2 s still) never got its samples.

## Mechanism (Basalt `~/src/basalt`, Monado `pimax_camera.c`)
- `pimax_on_imu` (libpimax session thread, `pimax_hmd.c`) -> `pimax_camera_push_imu` -> `xrt_sink_push_imu`
  -> `vit_tracker.cpp:447` `imu_data_queue->push` (VIO queue, **capacity 300, blocking**,
  `vio_estimator.h:91`) and `input_imu_queue.push` (optical flow, 3000).
- The VIO thread (`sqrt_keypoint_vio.cpp:246`) blocks on `vision_data_queue.pop` and drains IMU only per frame.
  In the 512 ms hole the VIO queue fills after 300 ms and the push **blocks the session thread**: no more IMU,
  no keepalives, no controller reports (and it holds `hmd->cam_lock`).
- First frame after the hole (8675.211): the optical flow (`frame_to_frame_optical_flow.h:146-190`
  `processImu`) blocking-pops IMU until a sample newer than the frame. The newest pushed sample is 8675.023, the
  next one is stuck in the session thread, which waits for the VIO queue, whose thread waits for the optical flow
  output. **Deadlock.** The image queue (capacity 10, `optical_flow.h:125`) then fills: 1 frame in the flow + 10
  queued = the 11 recorded frames, and the 12th `xrt_sink_push_frame` blocks the camera thread forever.
- With AE=0 there is never a frame gap > 300 ms, so this never triggers (94 s run fine). Any other camera gap
  > 0.3 s (USB hiccup, broker restart) would hang the same way.

## Fix (Monado `pimax_camera.c`, `slam_imu_gate_locked`)
Per frame set handed to the tracker, feed it at most 200 IMU samples at full rate, then 1 in 10 for up to 60
more (coarse IMU across a gap up to ~0.8 s), then nothing until the next frame set; nothing before the first frame
set. The VIO queue stays below 300, so the session thread never blocks. The Rust predictor and the EuRoC recorder
still get every sample. A gap logs `camera gap: holding IMU back from the tracker ...`; the 5 s `slam:` line shows
`N held ... in M camera gaps`. Expected after the first AE push: one gap warning, a pose jump at most, then
tracking continues.

Not changed: the AE frame, its timing, the Rust controller. Possible follow-up: send the first push before the
tracker starts (the re-init hole then happens before SLAM init).

## Live test (user)
After the relink: `sudo setcap cap_sys_nice+ep ~/src/monado-slam/build/src/xrt/targets/service/monado-service`,
then `PIMAX_CAM_AE=1 ~/pimax-re/analysis/tools/run_slam_view.sh 60`. Pass: one `camera gap` warning near the start,
`slam:`/`camera:` stats lines every 5 s through the run, camera ~50 fps, exposure in the AE status moving
into 1.1k-4.1k.
