# Camera auto-exposure: extended range in dim rooms and the "tracking light low" warning

The tracking cameras (OV7251, infrared) get little light from indoor LEDs. The exposure loop (`rust/pimax-cam/src/ae.rs`,
HID `0x51`, on with 6DoF) normally stays inside the largest values Windows' AE uses: **4108 us, gain 215**. After dusk it
sat at that ceiling with an image mean of 5-10 and Basalt tracking degraded. Windows Pimax Play holds the power-on
**10410 us / gain 449** (about 5x the light) all session (`analysis/tracking-vs-windows.md`, `slam-exposure.md`).

## Automatic extended range
- Armed at start: the Monado log has one `PIMAX_AE_EXTENDED: armed, ...` line (the build marker for this feature).
- **Extend:** the loop sits at its ceiling and the trimmed image mean is below 20 for 5 s. Stage 1 raises the gain cap
  to 449; if that is not enough, stage 2 (another 5 s) raises the exposure cap to 10410. **Gain first, exposure last**
  (motion blur): the split uses the normal exposure cap (4108) until every gain code is used, only then exposure grows.
  Gain 449 is the camera's power-on value; no new register value is written, only the limits of the existing `0x51`
  push change. 10410 us is far below the 40 ms tracking frame period (25 Hz).
- **Back:** mean above 80, or the loop needs less than half of the normal ceiling at a mean of at least 40, for 8 s: back
  to 4108 / 215. A gap of more than 2 s in the tracking frames resets the timers.
- The controller LED frames (fixed 40 / 62) are never touched.
- Log lines: `PIMAX_AE_EXTENDED: range extended to stage N ...` and `... back to the normal range ...`.
- `PIMAX_CAM_AE_EXTEND=0` turns the automatic range off. If `PIMAX_CAM_AE_EXP_MAX` / `_GAIN_MAX` already set the caps to
  10410 / 449 there is nothing to extend. `PIMAX_CAM_AE_EXP_MAX` also becomes the soft (gain-first) exposure cap.
- Not yet verified live: needs an A/B in a dark room (Basalt pose quality at 4108/215 against the extended range).
  The Monado driver only has to log; it needs no change (`pimax_cam_ae_out.light` carries the stage and the warning).

## "Tracking light low" warning
When even the full extended range (or the normal ceiling, if the range is off) leaves the mean below 15 for 10 s:

> Tracking cameras see too little infrared light — daylight or an IR lamp helps

- In the headset as a WayVR toast (XSOverlay-format UDP message to 127.0.0.1:42069, the same channel omarchyvrd and
  omarchyvr-voice use; only while WayVR runs, i.e. desk/Work mode).
- In the Monado log as `PIMAX_AE_LIGHT_LOW: ...`, which the app reads (`monlog.rs`, `CameraLog.light_low`): shown on the
  Tracking page (Head) and on Home's session status (Camera light).
- Cleared (log `PIMAX_AE_LIGHT_LOW: cleared`) when the mean stays above 25 for 3 s, or the range was retracted.
- Room setup has its own light check before mapping (docs/ROOM-SETUP.md).
