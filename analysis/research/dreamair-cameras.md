# Pimax Dream Air (SLAM): tracking cameras, hand tracking, Linux feasibility

Date: 2026-10-13. Read-only research. No HID commands sent, pimax-keepalived not touched.

## 1. Public specs (web)

- **Camera count and placement:** four cameras on the headset for inside-out SLAM tracking, no base stations. Sources: https://www.scan.co.uk/products/open-box-pimax-dream-air-slam-vr-headset-2x-3840-x-3552-micro-oled-lenses-2x-6dof-controllers-53-ppd , https://simracingcockpit.gg/pimax-dream-air-slam-review/ , https://store.pimax.com/products/pimax-dream-air?country=BR
- **Pimax's SLAM claim:** proprietary system, four cameras, "over a million tracking points", paired with IMU. Sources: https://www.auganix.org/?p=17112 , https://pimax.com/blogs/blogs/pimax-launches-affordable-slam-tracked-micro-oled-vr-headset
- **Resolution, fps, mono/RGB/IR:** not found in any source I could read. The Pimax store page came back truncated (https://store.pimax.com/products/pimax-dream-air/). The repo's own decode is the best data: 4 x 640x466 8-bit grey, 50 fps (see section 2).
- **Hand tracking:** listed as SLAM-only on the Dream Air (store.pimax.com comparison). Source: https://mixed-news.com/en/pimax-introduces-hand-tracking-for-crystal-super-crystal-light-pc-vr-headsets/ (Crystal line, same family of announcements).
- **Ultraleap or other third-party hand module:** not confirmed. A search for Ultraleap returned nothing relevant.
- **Camera module maker:** not confirmed by any web source. The local USB descriptor says the camera is `34a4:0014`, manufacturer string "eYs3d Technology, Inc.", product "PimaxSensor" (see section 2). That points to eYs3d as the USB/ISP vendor, but this is a descriptor fact, not a marketing claim. The sensor part (OV7251 per the repo's calibration notes) is also from local data only.

## 2. Local inspection (read-only)

Commands: `lsusb -t`, `lsusb -v -d 34a4:`, `ls /dev/video*`, `v4l2-ctl --list-devices`, `v4l2-ctl -d /dev/video0 --all`.

- **Topology:** the camera `34a4:0014` is on bus 2 (USB 3.2, 5000M), port 4 of a hub. Driver `uvcvideo`. It exposes `/dev/video0` (capture) and `/dev/video1` (metadata, no formats), plus `/dev/media0`.
- **Interfaces:** IF0 is VideoControl (INT IN EP 0x83, 32 B, with XU). IF1 is VideoStreaming with iso IN EP 0x81 and alt settings 0..N, up to 2x1024 B per microframe.
- **Format:** one format, `YUYV` (guid YUY2) at 1280x832, 60 fps (alt 50 fps). The real layout is not YUY2: it is 8-bit grey, 2560x832 (4 tiles of 640 px), with the image in rows 24..489. Frame size 2 129 920 B matches exactly. Repo: `analysis/camera-frames.md`.
- **Vendor bulk/HID:** the Pimax HMD (`34a4:0044`) has IF0..IF3 HID plus a USB audio IF4/IF5. None is a camera stream. The `34a4:0044` IF3 interrupt is the firmware log. Dev 22 (`analysis/imu-camera.md` section 3) is a bulk IN EP 0x83 at ~120 Hz, 1242 B records, not decoded. It could be eye, hand or controller data, so it is the first thing to decode for hand tracking.
- **Already done on Linux:** `analysis/frames/linux/frame0_2560x832.pgm` was converted from `/dev/video0` (`analysis/linux-peripherals.md`). So raw UVC frames are readable on Linux today without Windows.

Repo grep (camera, SLAM, UVC, stereo, 6DoF, cam, pvr): relevant hits:
- `analysis/camera-frames.md`: full frame/timing decode. PTS 100 MHz, 50.03 fps, 12-byte UVC header per packet, embedded metadata row (u32 us timestamp, u16 frame counter, 4 exposures, 4 gains).
- `analysis/init-sequence.md` and `analysis/windows-linux-command-diff.md`: host command `0x51` (12.5 Hz, `01 32 AA AA BB BB 0d 00 0d 00 0d 00 0d 00 28 00 3e 00`) is exposure/gain push, and the log says `update_fsin_pwm current_hz 50` (cameras are frame-synced at 50 Hz). Windows sends no UVC control to the headset in the captures; the camera starts when the iso stream is opened.
- `analysis/customheadsetopenvr.md`: the Windows SLAM path (Pimax's PVR client SDK, closed) gets pose, controllers, eye tracking, hand tracking (32-bone skeleton in the PVR API headers), and the VST camera from the PVR runtime. No Linux PVR exists. The OpenVR driver does not contain camera code.
- `analysis/device-map.md`: camera on USB 3.2, ~60 transfers/s on EP 0x81, iso alt 9 = 32768 B/interval, burst 15 (used by Windows).
- `analysis/imu-camera.md`: `device_calibration` XML has 4 cameras (trackingA/B, Pi-controllerA/B, 640x480 OV7251, interleaved) with extrinsics, plus IMU data. Calibration file: `calib/device_calibration_P511X0X000000000001.xml`.
- No hits for "hand" in the Windows driver dumps. Hand tracking appears only in the PVR SDK description in `customheadsetopenvr.md`.

## 3. Feasibility on Linux

**Frame capture: feasible, already working.** The stream is standard UVC iso on uvcvideo. Remaining work is only to reshape the bytes: `YUYV 1280x832` raw -> 8-bit grey `2560x832`, drop metadata rows 0..7, crop rows 24..489 into 4 tiles. Timestamps come from the metadata row (us, same clock as IMU) and the 12-byte UVC PTS.

**Open issues for the frame path:**
- Whether uvcvideo needs the `0x51` exposure push or the camera FSIN sync for steady 50 fps (unverified on Linux; Windows sends it).
- Whether the iso alt setting must match Windows (alt 9, 32 KB/interval) for stable frames. Linux currently uses the default alt chosen by uvcvideo.

**Hand tracking: feasible in principle, the hard part is the model.**
- There is no Pimax hand model on Linux. Pimax's tracking/hand stack is closed PVR code on Windows.
- We would need our own hand detector/pose model on 4 grey views (OV7251, fisheye per calibration; mono, no RGB). Options: MediaPipe/ONNX hand landmarks on each tile, then triangulation with the calibration extrinsics. This is a computer-vision project, not a protocol one.
- Quality risk: grey mono at 50 fps and wide FOV. Hands must be inside the overlapping FOV of two or more cameras for triangulation.
- If the headset itself returns hand data over USB (dev 22 bulk, 120 Hz, undecoded), hand tracking might not need the cameras at all. Decode dev 22 first.

**Rough difficulty:** frames = low (done). Calibration-based triangulation = medium. A usable hand model = high (weeks, model and data work).

## 4. Windows-side capture needed

Goal: find out how PiServer/Pimax Play opens and configures the camera, and whether hand data is sent over USB.

1. **USBPcap with full snaplen** on the camera (`34a4:0014`, bus 2) and the headset (`34a4:0044`). Snaplen must be at least 2.2 MB (about 106 MB/s, so ~1 s of capture per file). The earlier captures used 65535 and stored only ~3% of iso data (`analysis/camera-frames.md`).
2. **Capture the UVC setup**: VideoControl requests (GET/SET_CUR for probe/commit on IF1, XU selectors) and SET_INTERFACE alt values. These tell us the exact alt/bandwidth Windows uses.
3. **Two sessions with SLAM on**: (a) hand tracking off, (b) hand tracking on. Diff the dev 22 bulk stream and any new HID reports on `34a4:0044` to see if hand data comes from the headset.
4. **Optional**: PiServer with the camera running but the PVR client closed, to see if the camera stream alone carries on.
5. Save the pcaps under `captures2/` (existing convention). Do not run host HID commands from Linux during these captures.

Priority: (3) first, because it decides whether hand tracking needs the cameras at all. Then (1) and (2) to match Windows' camera setup.

## 5. Key findings

- Four OV7251-class mono cameras, 640x466 per tile, 8-bit grey, 50 fps, UVC iso on `/dev/video0`, already captured on Linux.
- Pimax's hand tracking is SLAM-only and runs in closed PVR code. No public vendor module (Ultraleap or otherwise) was confirmed.
- The camera's USB vendor string is eYs3d; the sensor/module maker was not confirmed on the web.
- Open question: does the headset send hand data over USB (dev 22)? Decoding it is the cheapest route to hand tracking.
