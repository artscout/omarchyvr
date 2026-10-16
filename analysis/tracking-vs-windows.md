# Head tracking vs Pimax on Windows (takes 8-12, 2026-10-15)

Tool: `omarchyvr-slam bench-windows TAKE [--root pimax-data/windows-20261015] [--predict-ms N] [--latency-ms N]`
(`rust/omarchyvr-slam/src/winbench.rs`). `WINBENCH_EXPLORE=1` adds the parameter sweeps below,
`WINBENCH_GYRO_SCALE=k` a gyro-scale A/B, `WINBENCH_DEBUG=1` the worst errors and every correction. Every
`PIMAX_SLAM_*` switch of the live output applies (the replay uses `HeadParams::from_env()`).
`omarchyvr-slam bench-basalt DATASET` bins Basalt's poses against the gyro by head angular speed (our Linux recordings).

## What the captures can and cannot give

* **IMU: complete.** 500 reports/s = 1 kHz samples with the MCU clock, every take. Clock fit: drift -20 ppm,
  USB arrival 1.06 ms (p50) / 2.1 ms (p99) after the fitted clock. The libpimax order (2nd sample = ts + 1 ms) is
  right (interleaved gyro 18x smoother than swapped).
* **Camera frames: not recoverable.** USBPcap kept only the first 64 KB of each 2 MB iso URB (3.7 % of the bytes),
  so Basalt cannot run on these takes. The benchmark therefore runs **our real output code** (`HeadTracker`:
  predictor, fusion, corrections, supervisor) on **the real IMU** with a **synthetic tracker** made from Pimax's
  own poses (lag removed, 25 Hz, 30 ms delivery, 2 mm / 0.05 deg / 0.02 m/s noise), queried at 90 Hz. Basalt
  itself is measured on our Linux recordings (below).
* **Pimax pose log:** 641 rows/s, `head_time == pvr_now` (asked for "now"). Timed by `head_time` mapped to the
  Windows clock (lower envelope); `utc_filetime` alone jitters by ms and made the first fit useless.
* Reference ("truth") = Pimax's pose shifted by its fitted lag and smoothed 5 ms (removes LibPVR's 90 Hz snaps).

## How Pimax tracks (answers "does Pimax predict? smooth?")

| | value | meaning |
|---|---|---|
| Lag of Pimax's "now" pose behind the IMU | **-2.1 ms (fast turns), +0.4 (walk), +1.4 (slow look)** | Zero lag: the pose is the IMU's motion up to the present (it slightly leads in fast turns) |
| Smoothing | **none** (best low-pass fit tau = 0 ms in every moving take) | No filtering of the rotation |
| State update | **~90 Hz** (head_w changes every ~11 ms) and constant-angular-velocity extrapolation in between | So Pimax *does* predict: from its last 90 Hz state to "now" (and the compositor further to the display) |
| Snaps at each 90 Hz update | 0.58 / 1.11 deg (p50 / p95) vs the smoothed trajectory in fast turns | Small steps hidden in motion |
| head -> IMU rotation | 0.04-0.38 deg (identity) | Pimax's head frame = our IMU frame |
| Gyro scale vs Pimax | 0.98-0.995 per axis | Regression attenuation, **not** a real scale error: correcting it (x1.015) made fast-turn error worse (0.84 -> 1.05 deg p95) |
| IMU position in the head frame | (-0.05, 0.0 ... 0.03, -0.04) m | Lever arm used by the replay |
| Jitter at rest | desk 0.008 mm / 0.000 deg, worn 0.16 mm / 0.025 deg RMS | Pimax holds a lying headset dead still |
| Exposure (AE command 0x51) | **10410 / gain 449 constant in all takes** | Pimax Play sent the power-on default the whole session (no AE). Our AE caps exposure at 4108 (2.5x shorter: less blur) |
| Fast turns (take 12) | gyro p95 422, max 637 deg/s; 9.7 s above 200 deg/s | |
| Pimax losses in take 12 | **4 losses (30.2 s +4.3 s, 35.7 +1.9, 40.0 +1.3, 41.6 +0.1), 82 % tracked, 12 jumps > 5 cm (max 19 cm)** | Pimax's own tracker loses 6DoF in the fastest turns |

## Our output vs Pimax, per take (before = current defaults; no live change was needed, see below)

Rotation / position error against the lag-free Pimax trajectory. "fast" = head > 150 deg/s. Position after
removing one constant offset per acquisition. "now" = queried for the present (as Pimax's log), "+20 ms" = with
display prediction as live.

| Take | Pimax now: rot fast p50/p95, lag | ours now: rot p50/p95 (fast), lag | ours +20 ms: rot fast p95 | ours pos p50/p95 (fast p95) | judder fast |
|---|---|---|---|---|---|
| 8 desk still | 0.00 deg, +2.6 ms | 0.40/0.43 deg (*) | - | 0.1/0.2 mm | - |
| 9 worn still | 0.00/0.01 deg | 0.40/0.44 deg (*) | - | 2.9/7.2 mm | - |
| 10 slow look (max 115 deg/s) | 0.24/0.25 deg, +1.4 ms | 0.41/0.50 deg (*), -1.7 ms | - | 4.7/9.4 mm | - |
| 11 walk (max 184 deg/s) | 0.06/0.11 deg, +0.4 ms | 0.18/0.30 (fast 0.22/0.34), +0.8 ms | 0.32 deg | 7.4/15.6 (9.4) mm | 0.05 deg |
| **12 fast turns (max 637 deg/s)** | **0.58/1.10 deg, -2.1 ms** | **0.21/0.77 (fast 0.30/0.84), +0.4 ms** | **1.12 deg** | 5.3/10.5 (**9.6**) mm | 0.20 deg |

(*) a constant 0.4 deg: the head -> IMU rotation for still takes comes from the slow-look fit (0.38 deg); a bench
offset, not an output error. Ours lost 6DoF twice in take 12, only where Pimax itself was lost (the synthetic
tracker follows Pimax's losses); max correction after re-acquire 2.8 deg / 0.39 m.

**Reading:** with a tracker as good as Pimax's, our IMU path and output layer are as fast as Pimax in fast turns:
lag +0.4 ms vs Pimax -2.1 ms, rotation error 0.30 / 0.84 deg vs Pimax's own 0.58 / 1.10 deg (its 90 Hz snaps).
The rotation side of "fast turns" is not where the gap is.

### A/B sweeps on take 12 (offline, `WINBENCH_EXPLORE=1`)

| Variant | rot fast p50/p95 | pos fast p95 | judder |
|---|---|---|---|
| current defaults | 0.30/0.84 deg | 9.6 mm | 0.199 deg |
| gyro scale x1.015 | 0.50/1.05 | 9.8 | 0.193 |
| tracker rotation noise 0 | 0.28/0.82 | 9.6 | 0.199 |
| rotation correction speed k_w 0 / 0.1 (default 0.3) | 0.30/0.86 / 0.30/0.84 | 9.6 | 0.191 / 0.199 |
| tau_rot 50 / 200 ms (default 100) | 0.28/0.87 / 0.33/0.92 | 9.6 | 0.210 / 0.193 |
| tracker latency 50 ms (default 30) | 0.31/0.88 | 11.6 | 0.198 |
| **tracker 50 Hz (default 25)** | 0.29/0.86 | **7.6** (walk 9.4 -> 5.5) | 0.199 |

None of the output parameters moves the rotation error by more than the reference's own noise, so **no default
was changed** (a change here would be unmeasurable). The only real gain is the tracker rate (position, -20..-40 %).

## Basalt in fast motion (our Linux recordings, `bench-basalt`)

Per tracker interval, Basalt's relative rotation vs the gyro's, binned by angular speed (6 recordings, AE on/off):

| Recording | peak | 150-300 deg/s: rot p50/p95 | > 300 deg/s | gaps > 60 ms |
|---|---|---|---|---|
| 151524 | 410 | 0.05/0.11 deg (10) | 2 intervals | 0 |
| 152215 | 359 | 0.04/0.11 (32) | 2 | 0 |
| 162452 | 277 | 0.05/0.15 (31) | 0 | 0 |
| 163703 | 348 | 0.04/0.41 (20) | 2 | 0 |
| 164420 | 240 | 0.06/0.12 (33) | 0 | 0 |
| 164606 | 486 | 0.07/0.40 (75) | 9 | 0 |

Basalt never dropped a frame or diverged in these, but they hold only seconds above 150 deg/s and almost nothing
above 300; take 12 has 5 s above 300 deg/s, where **Pimax's own tracker lost 6DoF 4 times**.

## Top causes of a fast-turn gap, ranked

1. **Vision in fast turns (unmeasured on Linux, the likely gap).** Pimax's own visual tracker loses 6DoF at
   400-640 deg/s with 10.4 ms exposure. Ours runs with AE (cap 4108, 2.5x shorter) or 10410 without AE; there is no
   Linux recording with deliberate fast turns yet. Needed: a Linux take like take 12 (`Record tracking`,
   scenario "fast turns", AE on), then `run_slam.sh replay` + `bench-basalt` + `omarchyvr-slam corrections`.
2. **25 Hz tracking frames** limit position accuracy between poses (fast p95 9.6 mm; 7.6 mm at 50 Hz). The
   cameras deliver 50 superframes/s alternating tracking/LED frames, so 50 Hz needs the LED frames usable for
   tracking (not now).
3. **Display prediction** (20 ms): fast-turn rotation p95 0.84 -> 1.12 deg, the same size as Pimax's 90 Hz snaps;
   constant-ω extrapolation like Pimax.
4. Not causes (measured): IMU rate/latency (1 kHz, 1.06 ms), camera/IMU clock (fit residual 1-2 % of signal at
   zero lag), sample order, gyro scale/misalignment (< 0.5 % and correcting makes it worse), IMU-head rotation
   (identity), output smoothing parameters.

## Open
* Record a Linux fast-turn take (AE on and off) and replay Basalt on it: that is the measurement that decides
  whether Basalt keeps tracking where Pimax loses it.
* A Windows recapture with full-size iso URBs (`snaplen >= 2.2 MB`, ~1 s) would let Basalt run on Pimax's own frames.

## Linux fast-turn take (2026-10-16)

`analysis/linux-recordings-20261016.md`: the live tracker held 6DoF through a Linux fast-turn take at up to 506 deg/s
(cameras, 40 ms average; 4.0 s above 300 deg/s) with 0 losses and no correction step over 5 cm, where Pimax's take 12
lost 6DoF 4 times (82 % tracked). Features that still match between frames at over 450 deg/s: median 124; motion
blur about 3 px at our AE exposure (about 1.7 ms) against about 20 px at Pimax's 10.4 ms. That answers cause 1 above
for the cameras; a Basalt replay of such a take needs the head IMU log that sessions write from the 2026-10-16 build
on (`omarchyvr-slam take-euroc`).
