# Windows Pimax 6DoF (SLAM) parameters, for our Monado + Basalt tracker

Read-only research. Source trees (not modified):
- `D` = `/mnt/shared/pimax-linux/windows/pimax-windows-dump-20261014-042210/files/` (Windows dump, 2026-10-14)
- `T` = `/mnt/shared/claude-omarchy-transfer/pimax-re/pimax-data/` (older copy; mirrors `D` for ProgramData/LocalAppData only, no ProgramFiles)

Headset: Pimax Crystal Dream Air, SN `P511X0X000000000001`, tracking build `Pimax_SLAM_1.9.9.8`, cameras `ov7251` 640x480, FISHEYE_4 model.

## 1. File inventory

| Path (under D unless noted) | Size | Modified | Note |
|---|---|---|---|
| `ProgramData-Pimax/slam/device_calibration_P511X0X000000000001.xml` | 3410 | 2026-10-14 04:17 (cal date 2026_07_28) | **Active calibration** (log: `input calibration_directory`) |
| `ProgramFiles-Pimax/Runtime/slam/device_calibration.xml` | 10382 | 2026-06-03 | Older generic calibration, different values |
| `ProgramData-Pimax/slam/P511X0X000000000001platMap_orborb_K8L4.bin` | 20603803 | 2026-10-14 00:56 | Saved SLAM map (ORB), used for relocalisation |
| `ProgramData-Pimax/slam/P511X0X000000000001platMap_orborb_K8L4.yaml` | 461 | 2026-10-14 00:56 | Map index: 3 submaps, 278/132/93 keyframes |
| `voc_GEN_8X4.dbow` | not found | - | Referenced in logs as `C:\Program Files\Pimax\Runtime\slam\voc_GEN_8X4.dbow`; the file is not in any folder on /mnt/shared |
| `ProgramFiles-Pimax/Runtime/slam/pimax_database.bin` | 4026080 | 2026-06-03 | Feature database |
| `ProgramFiles-Pimax/Runtime/slam/pimax_database_h2.bin` | 9188385 | 2026-06-03 | Feature database (H2 variant) |
| `ProgramData-Pimax/slam/6DOF_2026_10_13_23_34_34.txt` | 20587 | 2026-10-13 23:34 | SLAM run log |
| `ProgramData-Pimax/slam/6DOF_2026_10_14_04_17_25.txt` | 1831 | 2026-10-14 04:22 | SLAM run log (latest) |
| `session-logs-20261014/C__ProgramData_Pimax/slam/6DOF_2026_10_14_04_17_25.txt` | 45085 | 2026-10-14 04:28 | Full copy of the 04:17 run |
| `ProgramFiles-Pimax/Runtime/trackConfig.json` | 184 | 2026-07-17 | Tracking range/err config (see table) |
| `ProgramFiles-Pimax/Runtime/P3CONFIG.json`, `piservice.json` | 3315 / 11613 | 2026-07-08 | Service config, no SLAM keys found |
| `ProgramFiles-Pimax/Runtime/slam/H1/test_0|test_1/controller_params.json` | ~1.5 KB | 2026-06-03 | Controller model + ImuToDevice |
| `ProgramData-Pimax/slam/H1/*/controller_params.json` | 1540 / 1561 | 2026-08-28 | Controller model points (left/right) |
| `ProgramData-Pimax/slam/pimax_prior_position.txt` | 27 | 2026-10-14 | Prior position seed |
| `ProgramFiles-Pimax/Runtime/slam/models_and_config/` | dir | - | Hand model (`generic_hand_model.json`), no headset tracking params |
| `pimax-info.json`, `pimax-distortion-P511X0X000000000001.json` | 4698 / 2.8 MB | 2026-10-14 | Display distortion dump, not IMU/SLAM |

No Pimax SLAM DLL or EXE is present in the dump (`Runtime/` and `PVRHome/` have no `*.dll`/`*.exe`), so the `strings | grep` step was not possible.

## 2. Parameter table

Sources: `CAL` = `ProgramData/slam/device_calibration_P511X0X000000000001.xml` (active), `OLD` = `ProgramFiles/Runtime/slam/device_calibration.xml`, `LOG` = `6DOF_2026_10_14_04_17_25.txt`.

| Name | Value | File | Maps to Basalt / our config |
|---|---|---|---|
| Camera time offset (`TimeAlignment delta`, all 4 cams) | `0` | CAL | Basalt `cam_time_offset_ns` = 0 on Pimax's side. Our Basalt ignores it, so shifting data is needed (see `euroc.rs shift_cams`) |
| IMU-cam time offset (`Stateinit delta`) | `0.001638` s (1.64 ms) | CAL | Candidate for Basalt `td` (or our `timing.rs` offset) |
| Time offset, older file | `0.002444` s (`TimeAlignment`, cams A/B) and `0.002450` (controllers) | OLD | Same field in the older calibration; ~2.4 ms |
| IMU-cam rotation `tbc` (Stateinit) | `-0.006353 -0.014062 -0.019437` | CAL | Basalt `T_imu_cam` rotation seed (init) |
| IMU-cam rotation `tbc`, older | `-0.047455 -0.047086 0.004878` | OLD | Differs from CAL by ~0.05 rad; do not mix |
| Gravity-aligned init `ombc` | `-0.074171 -2.573029 0.628040` | CAL | Initial camera-to-body rotation vector (Basalt init) |
| Initial accel bias `aBias` | `0.079806 -0.022500 0.038815` m/s² | CAL | Basalt initial `accel_bias` |
| Initial gyro bias `wBias` | `0.003401 0.006936 -0.007014` rad/s | CAL | Basalt initial `gyro_bias` |
| Accel scale `ka` / gyro scale `kg` | `0.00616 0.00159 0.02118` / `0.00505 0.00299 0.00105` | CAL | Basalt IMU intrinsic scale/misalignment (Pimax fits these; check ours is calibrated) |
| Accel/gyro misalign `na`, `ng` | `0.00140 -0.01280 0.01563` / `-0.00231 0.00061 -0.00009` | CAL | Basalt misalignment terms |
| Gyro bias seed `ombg` | `0.000107 0.000858 0.000791` | CAL | Basalt bias init |
| `accelDelta` / `delta` (Stateinit) | `0.0` / `0.001638` | CAL | Same time-offset field as above |
| **IMU noise, active** (`IMUNoise`) | all `0.0` | CAL | Pimax ships **no noise model** in the active file. Our `ImuNoise::default()` (accel 4e-3, gyro 4e-4) is what we use |
| IMU noise, older (moving/stationary) | accel moving `1.296310`, gyro moving `0.022507`, accel stationary `0.081686`, gyro stationary `0.001974` | OLD | Pimax's own densities when they do fit noise. Stationary gyro 0.00197 is ~5x our gyro_noise_std 4e-4 (we are over-confident on gyro); the accel moving value is very large |
| Camera intrinsics (cam A) | fx 234.713, fy 234.706, cx 325.378, cy 248.283, FISHEYE_4, k = 0.05895 -0.06033 0.03203 ... | CAL | Basalt camera model `fisheye4`; use these, not defaults |
| Camera intrinsics (cam B) | fx 232.565, fy 232.503, cx 323.513, cy 246.602 | CAL | Same |
| Camera intrinsics (controllers A/B) | fx ~229.5 / ~233.9 | CAL | Not used by headset SLAM |
| Camera-to-rig (Rig, cam B) | t = `-0.09970 0.00710 -0.04973`, R = `-0.5778 -0.1162 -0.8079 / 0.1176 -0.9913 0.0586 / -0.8077 -0.0611 0.5864` | CAL | Camera extrinsic `T_imu_cam` (camera relative to IMU/rig). Check which Rig is the IMU frame before copying |
| Camera-to-rig (Rig, cam A) | identity (reference cam) | CAL | Reference camera |
| Cam calibration size / model | 640x480, `FISHEYE_4_PARAMETERS` | CAL | Basalt `resolution` / `camera_model` |
| `trackConfig.json` range | `[{Dist 45, start 0, end 2}, {Dist 40, 2-3}, {Dist 35, 3-4}, {Dist 25, 4-10}]`, `cacheNumber 3`, `errCountLimit 30`, `r 1.0`, `z0 2.0` | trackConfig.json | Their keyframe/feature-distance schedule. Our analogue: keyframe spacing (no direct Basalt key) |
| `Reloc param` | `4, 2, 18, 10, 20, 0.800000` | LOG | Relocalisation: unknown field names; 0.8 is likely a match-ratio/threshold |
| Loop/relocalisation mode | `loc_mode_ 0`; "Normal mode, load map to reloc, and save map" | LOG | Pimax loads the saved map at start and saves on exit |
| Gravity init | `x -0.014881 y -0.005800 z 9.806657 norm 9.806670` | LOG | Initial gravity estimate (we use 9.807 in `euroc.rs`, OK) |
| Feature/ORB counts, FAST, scale, levels | not in any text file | - | No value found. Only in the binary SLAM DLL, which is not in the dump |
| Exposure / AE settings | not in any text file | - | No value found in the dump |

Notes on the table: the active calibration file (CAL) is dated 2026-07-28 and has `IMUNoise` all zero, so Pimax's runtime clearly does not use the older noise values by default. The two XML files disagree on `tbc` and on all Rig translations, so we should first confirm which file is current before copying any extrinsic.

## 3. Log insights (`6DOF_2026_10_14_04_17_25.txt`, `session-logs-20261014`)

- Counts (all runs): `Not enough matched features: 21` x163, `22` x7, `23` x3, `26` x2, `49` x2, others x1. Tracking is feature-starved most of the time. This is the strongest hint in the logs: Pimax's own tracker also loses features often.
- `imu_vec size > 3000` x19: the IMU buffer is growing past 3000 samples, which suggests the IMU queue is backing up (rate or timing issue).
- `re-localization, load ...platMap_orborb_K8L4.bin success` x5: map load works and is used at start.
- `input vocabulary full path ...voc_GEN_8X4.dbow`, `voc_path ...\slam\`: DBoW vocabulary is loaded from Runtime/slam.
- `imu initial gravity estimator is good`: gravity-init succeeded.
- `Stereo tri: No features.` x1: one triangulation failure.
- `dof_conversion_timeline.txt` is empty: no 3DoF/6DoF switches were recorded.

## 4. The 3 parameters most likely to fix our drift (2.2 m standing, 23% loop drift)

1. **Camera-IMU time offset.** Pimax's active XML has `Stateinit delta = 0.001638 s` (and the older file 0.002444 s), with `TimeAlignment delta = 0` on the cameras. Our `timing.rs` and `euroc.rs shift_cams` already sweep offsets. Try **1.64 ms first, then 2.44 ms**, applied as a camera-timestamp shift. A 2 ms error at 1 m/s head motion is about 2 mm per frame, but standing still it does not explain 2.2 m, so this is a strong candidate for the loop drift and the yaw/scale error, not the standing drift alone.
2. **IMU noise density.** Pimax's active file has zeros, but the older file has stationary gyro 0.00197 and stationary accel 0.0817. Our defaults are `gyro_noise_std 4e-4` and `accel_noise_std 4e-3`, which are 5x and 20x tighter than Pimax's stationary numbers. Over-confident gyro/accel noise lets the filter trust a biased IMU and drift, which fits 2.2 m drift while still. Try **gyro_noise_std about 2e-3 and accel_noise_std about 8e-2** (Pimax stationary values) as a first step. Units must be checked against Basalt's per-sqrt(s) densities before the final value.
3. **IMU-camera extrinsics (`T_imu_cam`, Stateinit `tbc`/`ombc`).** The two calibration files differ by ~0.05 rad in `tbc` and up to ~0.1 m in Rig translation. If our extrinsics come from the older or a generic file, the rotation error alone gives large apparent drift. Use **the ProgramData XML values (CAL, 2026-07-28)** and confirm which Rig is the IMU frame.

Also worth checking, not in the top 3: the repeated "Not enough matched features" and "imu_vec size > 3000" lines suggest feature count and IMU queue depth; Pimax's ORB/FAST settings are not in the dump, so we can only tune our own counts empirically.

## 5. Open items

- `voc_GEN_8X4.dbow` is not on the shared drive; we cannot reuse Pimax's vocabulary.
- The SLAM binary (for ORB/FAST/exposure/feature counts) is not in the dump; a fresh dump of `Program Files\Pimax\Runtime\slam` would be needed.
- The `Reloc param` field names are unknown.
