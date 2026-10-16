# IMU orientation on Linux (tools: linux/pimax_calib.py, linux/pimax_imu_orient.py)

Measured live 2026-10-13, headset sitting on a desk, keepalived running, IF2 read-only.

## 1. Calibration read over IF0
`linux/pimax_calib.py` sends only `58 07 0a 00 01 AL AH 10` (84x, AL:AH 0x0000..0x0530), filters replies
`4c .. 0a 00 01 AL AH 10 <16 B>` by address, inflates `blob[16:16+u32le(blob[0:4])]`.
Result: 1344 B read, zlib 1318 B, XML 3410 B, saved as `headset-calibration-linux.xml`.
**Byte-identical to the Windows capture (`headset-calibration.xml`, `cmp` clean).** Read works while the keepalive
service is running; no retries were needed.

## 2. IMU decoding (confirmed on live data)
Report on IF2: `53 31 08 00 | u32 LE ts_us @4 | sample0 @12 | sample1 @28 (ts+1000 us)`.
Sample (16 B): be16 temp @+0, be16 acc xyz @+2, be16 gyro xyz @+8. acc 2048 LSB/g, gyro 16.4 LSB/dps.
|a| = 0.9989 g at rest, so the accel scale is right. The device clock ran at the wall clock rate (500 reports/s,
no gaps seen in 120 s). Timestamp wrap (2^32 us = 71.6 min) is handled by the tool.

`pimax_imu_orient.py` = Mahony filter (Kp 1, Ki 0), 1 kHz, dt from the device timestamp, accel gated when
|a| is not within 0.7..1.3 g or the gyro exceeds 150 dps (sum of |axes|). The initial tilt is seeded from the accel.
Modes: default (text ~10 Hz, `--rate`), `--json` (`{"t_us","q":[w,x,y,z]}`, q = sensor->world, world up = +Y),
`--still N`, `--auto-bias S`, `--no-calib`, `--kp/--ki`.

## 3. Factory biases do not match the real sensor
Measured gyro mean at rest (120 s, raw dps): x +0.303, y +0.729, z +0.425.
XML `wBias` converted to dps: x +0.195, y +0.397, z **-0.402**. Same order of magnitude, but z has the opposite
sign and y is off by 0.33 dps. Removing the XML bias leaves drift of 6.5 / 19.9 / 49.6 deg/min (x/y/z), which is
worse than doing nothing on z. Possible causes: `wBias` is expressed in the camera/body frame given by `ombc`, the
sensor bias changed with temperature since the 2026-07-28 calibration, or the bias is a different quantity.
Do not use the XML bias directly; measure the bias at startup (`--auto-bias`, 10-20 s of stillness) and keep a
running estimate (Ki, or zero-velocity detection). Gyro bias is not rotated into the sensor frame by the tool.
The XML scale terms `ka`, `kg` are about 0.5-2 percent; they are applied as 1/(1+k), the tool ignores `na`, `ng`
(non-orthogonality, about 0.001-0.015). The accel XML bias (0.08, -0.02, 0.04 m/s^2) moves static tilt by 0.1-0.4 deg.

## 4. Noise and drift (120 s still)
| | x | y | z |
|---|---|---|---|
| gyro noise 1 sigma, 1 kHz | 0.047 | 0.062 | 0.062 dps |
| angle random walk | 0.0015 | 0.0019 | 0.0020 deg/sqrt(s) |
| bias change between first and second 60 s | -0.019 | -0.066 | +0.016 deg/min |

Yaw (about Y = up) with no magnetometer:
* No bias removal: **43.7 deg/min** (raw bias 0.73 dps).
* XML bias removed: 19.9 deg/min (wrong).
* Bias measured over a 20 s window at start (`--auto-bias 20`, then 95 s run): yaw moved -0.17 deg over 95 s, i.e.
  **about -0.1 deg/min**. The bias estimation error with a 20 s window is about noise/sqrt(20000) = 0.0004 dps (0.03 deg/min);
  the dominant term is real bias instability (about 0.07 deg/min between halves, temperature slowly changing).
* Expect 0.1-0.5 deg/min once warm and calibrated; long-term, the SLAM cameras must correct yaw.

Pitch/roll: the filter output was 5.17 / 2.01 deg and the stationary accel tilt (after XML accel bias) was 5.15 / 2.02 deg,
stable to 0.01 deg over 95 s. The absolute accuracy is limited by the unknown mounting level of the desk (the table is not a
reference), so the accuracy against a true horizon is unverified; expect about 0.5 deg from accel bias/scale error
(difference raw vs XML-corrected accel is 0.1 deg pitch, 0.5 deg roll).

## 5. Axis convention
Evidence:
1. Gravity: accel reads about +1 g on **+Y** at rest (headset on its back/level; (-0.027, 0.995, -0.088) g now,
   earlier raw (-40, 1922, 690)). Specific force points up, so **+Y = up** (high confidence).
   Gyro axes equal accel axes (imu-camera.md LSQ fit), right-handed, positive = counter-clockwise looking down the axis.
2. Yaw = rotation about Y, pitch = about X (gy and gx dominate in the Windows capture). Both consistent with +Y up.
3. Extrinsics: `ombc` = (-0.0742, -2.573, 0.628) rad, a rotation vector of 151.8 deg, R =
   [[-0.880,-0.061,-0.471],[0.163,0.893,-0.420],[0.446,-0.446,-0.776]], `tbc` = (-6.4,-14.1,-19.4) mm.
   Camera axes (trackingA frame, OpenCV: x right, y down, z forward) expressed in the IMU frame (columns of R, assuming R maps cam->IMU):
   x_cam ~ (-0.88, 0.16, 0.45), y_cam ~ (-0.06, 0.89, -0.45), z_cam ~ (-0.47, -0.42, -0.78).
   (Using R^T instead puts the IMU axes in camera coordinates and gives the same Y/Y alignment, 0.893.)
   The 152 deg angle is about 180 deg about Y plus a roughly 25-30 deg yaw and pitch of the camera: the camera optical axis is
   mostly along IMU **-Z** (0.78), tilted sideways and down, and cam x is mostly **-X**.
   A forward-looking camera with image-down aligned to IMU +Y (up) means that the OV7251 tracking camera is mounted upside-down
   (rotated 180 deg about its optical axis), which is common. Under that reading: forward = IMU -Z, right = IMU +X, up = IMU +Y.
   Cross-check: trackingB sits at x = -0.0997 m in the trackingA frame, which (cam x = -IMU x) puts B on the right.

Proposed rotation sensor -> OpenXR (+X right, +Y up, -Z forward): **identity** (R = I, no axis flips).
Confidence: Y up high (~97 percent); handedness is fixed by Y and the right-handed gyro; X right / Z back about 60 percent,
because it relies on the cameras being forward-looking and the camera being flipped, and the Windows capture never moved
the headset enough to settle it. The alternative is a 180 deg rotation about Y (X left, Z forward).
Cheap confirmation: tilt the headset nose-down by about 30 deg while running the text mode. With the identity mapping, pitch
must print negative; yaw to the left must print positive; tilting the right side down must print positive roll.
Flip signs of X and Z (quaternion x,z) if the output is inverted.

## 6. Commands
```
python3 linux/pimax_calib.py                      # read + diff calibration
python3 linux/pimax_imu_orient.py                 # yaw/pitch/roll 10 Hz (calibrated scale, XML bias)
python3 linux/pimax_imu_orient.py --auto-bias 15  # keep the headset still for 15 s first
python3 linux/pimax_imu_orient.py --json --rate 100
python3 linux/pimax_imu_orient.py --still 60      # bias / noise / drift report
```
Caveat: default mode applies the XML bias unless `--auto-bias` replaces it with measurement. Prefer `--auto-bias`.


## Tilt test result (2026-10-13)
User tilted the headset ~40 deg nose-down: identity mapping reported pitch +39 deg (OpenXR: nose-down is negative).
So sensor X and Z are flipped relative to OpenXR: use axis mode 1 (180 deg about Y; X and Z negated). The Monado pimax driver now defaults to PIMAX_AXIS_MODE=1.
