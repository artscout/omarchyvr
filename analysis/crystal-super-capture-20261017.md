# Pimax Crystal Super (Micro-OLED): first Windows capture, 2026-10-17

Capture: Pimax Play 2.0.2.103 (PVR 1.33.1), USBPcap 1.5.4, all root hubs, headset on hub 2. 14 takes (cold start x2,
idle, IPD, settings, controllers, sleep/wake, shutdown, eye calibration, lighthouse x3, SLAM x2) plus PVR dumps
(distortion 129x129 per eye, info, data files) and pose logs. Raw files stay off the repo (they hold serials).
Decoded with a USBPcap reader (non-isochronous packets only, device address mapped to VID:PID from GET_DESCRIPTOR).

## USB devices of the headset

| VID:PID | What | Dream Air counterpart |
|---|---|---|
| `34a4:0040` "Pimax Crystal Super" | 4 HID interfaces (64-byte reports, no report ID; IF0 has the OUT endpoint) + USB audio | `34a4:0044` (same interface layout) |
| `34a4:0041` "Pimax Device" | 1 HID interface; only sends ASCII log lines during the capture, the host sends nothing | `34a4:0043` bridge |
| `34a4:0014` eYs3d "PimaxSensor" | UVC tracking cameras | same ID |
| `2104:0561` | Tobii eye tracker (powered by `58 05 3c 05 00 01`) | same ID |

The model is reported as `hmd_codename Pimax P3C10 Micro-OLED`; `58 04 3a 05 01` answers module type `0x05`
(= Micro-OLED in CustomHeadsetOpenVR; 0x11 57PPD, 0x22 50PPD, 0x44 Ultrawide). PVR: 7104x3840 window, 89.99986 Hz,
IPD 63.19 mm, `SecondsFromVsyncToPhotons 0.00889`. The hidden-area mesh from PVR is empty.

## Protocol: the Dream Air protocol

Every command is a `58 LL CMD OPT ...` frame on IF0 EP 0x01, replies `4c ...` on EP 0x81, exactly as on the Dream Air
(docs/HEADSET-COMMANDS.md, analysis/init-sequence.md). No HID feature reports; control transfers are only the
standard descriptor and string reads. Cold start (take `cold2`, t = seconds into the capture):

| t | Bytes (rest 0) | Meaning (from the Dream Air work) |
|---|---|---|
| 21.339 | `58 06 1a 05 01 00 00` | wake / PC alive (reply `4c 04 1a 05 01 07`) |
| 21.39-21.48 | `58 04 16 00 1e`, `58 03 03`, `58 03 09`, `58 03 50`, `58 03 19`, `58 04 3a 05 03/01/00`, `58 04 56 05 0a`, ... | identity, versions, module type |
| 21.43 | `58 04 39 00 00 00`, then from 24.9 `58 04 39 00 5a 00` every ~220 ms | keepalive, with rate 0x5a = **90 Hz** (Dream Air: 0x48 = 72) |
| 24.95-25.15 | `58 0b 27 05 'lens_rd'` x4 | config key read |
| 25.30-39.46 | 96x `58 07 0a 00 01 AL AH 10` | calibration read, 16-byte chunks |
| 25.962 | `58 0d 1a 05 02 5a` | required fps = 90 |
| 26.058 | `58 0d 1a 05 05 00` | GPU vendor (NVIDIA = 0) |
| 27.543 | `58 05 3c 05 00 01` | power the eye module |
| 27.88 → | `58 19 51 00 01 32 ...` (1132 in the take) | camera exposure / LED sync |
| 39.42-39.48 | `58 04 16 03/02 1e`, `58 03 50 03/02`, `58 03 03 02/03` | start the controllers (slots 2 and 3) |
| once | `58 05 5b 05 02 02`, `58 04 29 00 01` | app_unlock, panel-on request |

No encrypted activation blob (`0x55`) was sent in the cold start.

Input reports are the Dream Air ones: IMU `53 31 08 00` on EP 0x83 at ~500 reports/s (2 samples each),
controllers `53 32 08` on EP 0x82 at ~500/s, firmware log text on EP 0x84 (ICM42688 IMU).

## What this means

- libpimax / pimax-proto / pimax-capi can drive this headset with the same frames. The differences found so far are
  the USB product ID (0x0040), the bridge ID (0x0041), the video rate byte (0x5a), the display mode and the optics.
- Done (2026-10-17): `pimax_proto::model` lists the models (Dream Air 0044/0043, Crystal Super 0040/0041);
  hidraw discovery (Rust and C), the component probes, preflight, supervise and keepalived find either; the Monado
  prober has a `Pimax Crystal Super` entry and names the HMD after the model; the app lists and pre-selects it;
  `devices/crystal-super/device.toml` is a real, experimental module whose allowlist holds only captured frames.
  Restart headset refuses the Crystal Super (its restart frame was not captured).
- The display mode on the GPU is the same 7104x3840 window as the Dream Air, so the NVIDIA notes for that mode
  (analysis/nvidia-vk-display-2h1or.md) apply until shown otherwise.

## Still open

- The IMU axis mounting and the camera layout of the Crystal Super (compare with the PVR info and the pose logs).
- What `0x0041` is for, beyond its log.
- Lens: fit `pimax-lens fit-pvr` on the dump; CustomHeadsetOpenVR (GPL-2.0) has a "Crystal Super Micro-OLED Default"
  profile to compare against (reference only, not copied).
- Other takes, first pass: IPD motor `58 06 5c 05 01 <mm*100 LE>` (66.00, 63.19, 60.00), rate change
  `58 04 1a 05 04` + keepalive 0 then `58 0d 1a 05 02 48` + keepalive 0x48, standby on removal: keepalive 0 then
  `58 0d 1a 05 03`, wake back: `02 5a`, `05`, keepalive 0x5a; eye module off `58 05 3c 05 00 00` (eye tracking
  switched off). Brightness and contrast send no HID frame. Lighthouse and SLAM takes not decoded yet.

## First run from Linux (2026-10-17 23:16, Ubuntu 26.04, KDE Wayland, RTX 5090, NVIDIA 595.99.02)

- Before: with no host keepalive the HID chip `34a4:0040` re-enumerated every ~5-6 s (device numbers counting past 100),
  the same symptom as the Dream Air without keepalive. The bridge `0041`, the cameras `0014` and the Tobii `2104:0561`
  stayed on USB.
- `omarchyvr keepalived` (wake `58 06 1a 05 01 00 00`, then `58 04 39 00 00 00` every 213 ms; nothing else) found the
  headset on its first try ("headset on /dev/hidraw18 (Pimax Crystal Super)"). The re-enumeration stopped.
- The headset's DisplayPort connector appeared (card1-DP-6 connected) within seconds, without any video-prepare frame.
  EDID: manufacturer `PVR`, product 4121 (0x1019, the same as the Dream Air), name `Pimax`, DisplayID timings
  7104x3840 at 89.999868 Hz (2564.21 MHz) and 72.404905 Hz (2028.00 MHz). KDE did not add it as a desktop output
  (the kernel treats it as non-desktop), so it is free for a DRM lease. No Xid or nvidia-modeset errors.
- IMU on IF2: 506 reports/s, `53 31 08 00` layout decodes as on the Dream Air (at rest about 1 g on one axis);
  IPD readback at bytes 45..46 = 6319 = 63.19 mm, the value set in Pimax Play.

## Why the GPU offers no mode (2026-10-17 23:40, RTX 5090, NVIDIA 595.99.02 open, Ubuntu signed modules)

- Monado (direct mode on DP-6 through KWin's `wp_drm_lease_device_v1`) stops at "Found no connectors available for direct
  mode": KWin leases nothing because nvidia-drm lists zero modes for the connector (`/sys/class/drm/card1-DP-6/modes` empty).
- NVKMS `VALIDATE_MODE_INDEX` with an info string (read-only client, same method as analysis/nvkms-mode-validation.md;
  the device instance is 0, not the RM gpu id 0x100; the info buffer must be <= 2048 bytes) on the headset dpy:
  - 7104x3840 @ 90 Hz (2564.21 MHz) and @ 72 Hz (2028.00 MHz): "DSCPassThrough: No", "Mode is rejected: DP Bandwidth
    check failed." The EDID range limits (55-65 Hz, 50-70 kHz) only produce "contradicts itself" warnings.
  - `dscMode = FORCE_ENABLE`: the same rejection.
  - `NO_DISPLAYPORT_BANDWIDTH_CHECK`: both modes VALID with "DualHead Mode: No" (one head).
- Reading of the 595.99.02 source: `nvEvoUse2Heads1OR` (nvkms-evo.c:9813) picks 2Heads1OR only when the pixel clock is
  above the head's `maxPClkKHz`; a Blackwell head takes 2564 MHz, so the mode stays on one head. Single-head DSC limits
  `pic_width` to 5120 (nvt_dsc_pps.c:1013), the panel is 7104 wide, so DSC cannot be used, and uncompressed 7104x3840@90
  at 24 bpp (~61.5 Gbit/s) does not fit the link. On the Dream Air's RTX 4090 (Ada) the pixel clock was above the head
  maximum, so 2Heads1OR (dual DSC, width up to 8192) came by itself.
- Fix candidate: a patched nvidia-modeset whose `nvEvoUse2Heads1OR` also returns TRUE when the visible width is above
  5120 (DSC needed and possible), plus whichever of the Dream Air's three 2Heads1OR patches apply to 595. Unverified:
  whether the sink advertises DSC (`nvDPDpyIsDscPossible`), which needs a DPCD read (root).

## First picture (2026-10-18 00:41): Monado direct mode, 7104x3840 @ 90 Hz, head-locked test grid seen in the headset

What it took on the RTX 5090 (Blackwell) with NVIDIA 595.99.02 open modules (patched tree: local branch
`omarchyvr-crystal`, not in this repo yet):
- Blackwell has no 2Heads1OR (nvkms-evo4.c `supportsMergeMode = FALSE`); it does not need it: DSC on Blackwell is
  "multi_tile" (`maxNumHztSlices > 4`), which allows a 7104-wide picture on one head. The Dream Air 2Heads1OR patches
  (and a VR-headset 2Heads1OR rule) do nothing here.
- Blocker 1, DP library: the sink's FEC capability (DPCD 0x90 = 0xbf) is read once at detection, while the headset's
  AUX channel still fails ("AuxChCtl Failing"); the cached "no FEC" made `compoundQueryAttachSSTIsDscPossible` false.
  Fix: re-read FEC there when the cache says no.
- Blocker 2, EDID: base block byte 20 = 0x85 (DisplayPort, colour depth undefined) -> NVKMS validates at 6 bpc, and
  DSC refuses 6 bpc. Fix: an EDID override (nvidia-modeset `config_file`, key `override.[0000:01:00.0].DP-4`) with
  byte 20 = 0xA5 (8 bpc) plus a CTA block with only a Microsoft VSDB v2 (primary use 7 = VR headset), which makes the
  kernel mark the connector non-desktop: KWin does not use it as a monitor and leases it to Monado. The headset's
  DisplayID 1.2 timings stay unchanged. (debugfs `edid_override` is refused under Secure Boot lockdown.)
- Result: both modes VALID (one head, DSC, HBR3 x4, FEC); KWin lease granted; modeset "pclk 2564210 KHz raster
  7328 x 3888, 129 bpp (x16), DSC Mode = SINGLE"; `xr_grid` rendered at 90 Hz and was seen in the headset; no Xid.
  Both controllers sent reports (slots 2 and 3).
- Lesson: a diagnostic print that called `compoundQueryAttachSSTIsDscPossible` without a native device crashed the
  boot (NULL dereference) once the EDID override made modes appear before the DP library had found the sink.

## Lens and 3DoF check (2026-10-18 00:45-00:53)

- Lens: `pimax-lens make-lut` on this headset's PVR dump (129x129 grid, 2 eyes x 3 channels; bilinear vs the 1024-sample
  radial lines rms 0.0032 deg, max 0.035 deg); FOV per eye L53.82 R47.41 U48.67 D48.67 deg (left eye), cant 2 deg.
  The radial fit is not good enough (`fit-pvr`: rms 1.14 deg, "NOT radially symmetric"), so the LUT mode is used:
  `lens.conf` = `cant_deg = 2` + `lut = <file>`. In the headset: grid lines straight, circles round.
- Head-locked grid "breathes" a little when the head turns or tilts (pupil swim of a static distortion); a world-locked
  room (`xr_room`) shows no such effect. Pimax Play corrects it with eye-tracked dynamic distortion (later work).
- 3DoF with the Dream Air axis mapping (`PIMAX_AXIS_MODE` 1): `posecli` shows clean yaw (+-78 deg), pitch (+-55 deg),
  roll near 0 during pure yaw or pitch, the rest pose returns to the same yaw (no drift seen in 3 min); in `xr_room`
  the room stays put and every direction is right. Both controllers report (slots 2 and 3).

## Cameras and controller tracking (2026-10-18 01:00-01:12)

- Cameras `34a4:0014`: the same UVC stream as the Dream Air (YUYV 1280x832 at 60/50 fps = 2560x832 grey, four 640x480
  OV7251 tiles); `pimax-cam info` parses the metadata unchanged: frames alternate tracking (type 0x86, firmware
  default exposure 3500, gain 60, tile rows 100) and LED (0x06, exposure 25, gain 35, rows 9) at 60 fps.
- Factory calibration (Windows `slam/device_calibration_<serial>.xml`): the Dream Air format and camera names
  (trackingA/B, Pi-controllerA/B, FISHEYE_4_PARAMETERS, 640x480, f ~234 px); only the rig differs: tracking cameras low
  in front at x = +-37 mm looking down ~38 deg and +-10 deg sideways, controller cameras on the sides at x = +-85 mm,
  yaw +-35 deg, pitch +20 deg.
- Controller LED models (`slam/H1/<serial>/controller_params.json`): identical to the Dream Air reference
  (15 points per hand, max difference 0.0): the controllers are the same.
- Pimax Play camera exposure frames `58 19 51 00 01 32 ..` (16751 in the takes): exposure e0/e1 2200..10410,
  e2/e3 2200..8941, gain 13..449, LED exposure 40 / gain 62 always (Dream Air allowlist: 100..4108, 13..215).
- Offline `pimax-ctrltrack track` on a 15 s recording (both controllers in hand) with the Crystal calibration and the
  factory extrinsics: both controllers in 85.9 % of 434 LED frames (none in 12.2 %, arms lowered), joint reprojection
  RMS median 1.32 px (p99 1.55), median 10 LEDs per pose, left/right right in 746/746 poses, median step 1.3 mm.

## Inside-out 6DoF in SteamVR, first try (2026-10-18 01:17-01:22)

Camera broker + `pimax-ctrltrack live --factory` + SteamVR `driver_monado.so` with `PIMAX_SLAM=1 PIMAX_CTRL_TRACK=1`,
the Crystal calibration (`PIMAX_CAM_CALIB`), Basalt df6e970 with `omarchyvr-slam export` from that calibration,
camera auto exposure off (firmware default exposure 3500 / gain 60).
- Controllers: tracked optically and followed the hands; they looked a little higher than the real hands.
- Head: unstable. Monado: "6DoF: lost (position jump)", 25 tracker resets in about 2 min, head speed estimates of
  4 m/s while nearly still, gyro bias estimate wandering to 0.9 deg/s; the user sank through the floor.
  Candidates: the Crystal's IMU-camera extrinsics convention, the camera timing (60 fps, tracking frames at 30 Hz;
  the Dream Air ran 50 fps), the zero IMU noise in the factory file (defaults used), the dark images.
- Floor at eye level: the driver's origin is the start pose; SteamVR treats height 0 as the floor (room setup or
  the anchor floor is needed, as on the Dream Air).
Paused in favour of the Lighthouse faceplate (the user owns 3 base stations and Index controllers).

## Lighthouse mode, the light setup, audio (2026-10-18 01:25-01:58)

- Lighthouse faceplate switched on by hand: `28de:2101` x2 (Watchman dongles) and `28de:2300` "Valve LHR" appear on
  their own (no HID command needed on Linux; Pimax Play sends `58 0d 26 00 'curfms' 00 00 01 01` + `58 04 2b 00 02`
  when it switches, `... 00 00 01` + `2b 00 03` back; not sent by us).
- SteamVR's lighthouse driver tracks the headset and Index controllers; the LHR config has a 1x2 px render target and
  no EDID, so CustomHeadsetOpenVR (sboys3) shims the display: on Linux its HID hooks are compiled out ("hid functions
  are not actually exported on Linux"), and the Pimax shim was never chosen because the direct USB connection that
  identifies the headset is only started by those hooks or by the shim itself. One-line fix: call
  `PimaxCommon::TryDirectConnection()` before `IsLighthouseHeadsetConnected()` when `PVR_EXISTS` is not defined.
  Then: render target 4125x3306 per eye, output viewport 3552x3840 per eye, "Acquired drm display!", direct mode,
  SteamVR Home in the headset; works when SteamVR is started from Steam's own button too.
- The light setup: SteamVR (lighthouse) + CustomHeadsetOpenVR + `omarchyvr keepalived --video 90` (new option: the
  video-prepare frames at every connect, 2 s after the wake) as the user service, the patched NVIDIA modules and
  the EDID override. Monado is not used in this mode.
- The "water running down glass" artefact near the centre of both eyes is SteamVR Home's own: Beat Saber and the
  Falcon BMS menu are clean. Ruled out on the way: DSC (`dsc_slice_height` 240 instead of 1920 changed nothing) and
  CHOVR's RadialBezier profile (probed offline around the lens centre: slope ripple below 0.1 %, no fold; SteamVR
  samples it on a 127 mesh). Best profile in the headset: "Crystal Super micro-OLED like Pimax".
- Open: about 4 % dropped frames, no reprojection ("Async: Automatically disabled" on this Vulkan setup).
- SLAM data (236 s EuRoC recording, 2 cameras at 30.0 Hz with no drop, IMU at 1012 Hz with no gap): |a| in quiet
  windows is 10.11 m/s^2. The factory calibration's `Stateinit` holds the IMU biases: `aBias` (y +0.343 m/s^2, the
  gravity axis) brings |a| to 9.79; `wBias` z 0.0233 rad/s matches the measured gyro mean (0.0235). The Basalt export
  still writes zero biases; `delta` (1.58 ms) is a candidate camera-IMU time offset. `ka`, `kg`, `na`, `ng` are not
  per-axis scale factors (applying `ka` as one makes |a| worse).
- Audio: USB audio on the HID device, not DisplayPort (the EDID has no audio block). Speakers are ALSA device 1
  (`hw:Super,1`, S16_LE 2 ch 48 kHz, interface 7), the microphone device 0 (interface 5); PipeWire's analog profiles
  only probe device 0 for output, so only the microphone showed. A WirePlumber rule picks `pro-audio` for the card
  and names the nodes; confirmed by ear.
