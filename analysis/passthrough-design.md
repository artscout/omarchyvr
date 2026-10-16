# Camera passthrough for the Dream Air on Linux: design and first version (2026-10-14)

**Status:** first version built and checked offline, not yet run on the headset. `rust/pimax-passthrough` (Rust
crate plus CLI) renders correct per-eye passthrough images from recorded frames. It also contains an OpenXR client
that shows them in the headset with IMU rotation compensation. **Monado is unchanged.** No hardware was touched:
no camera, HID, monado-service or OpenXR runtime. Live test commands are in section 7.

## 1. What we have to work with

* Cameras (`analysis/overnight/cam.md`): one UVC stream with four grey 640x480 KB4 fisheye tiles at 50.03 fps. Tracking
  and LED-exposure frames alternate, so the scene comes at **25 Hz**. Tiles 0 and 2 are mounted upside down. Each
  frame carries an MCU timestamp in µs, on the same clock as the IMU.
* Factory calibration: KB4 intrinsics plus the rig and camera-to-IMU extrinsics. The morning check (pair 0-1, `conv2 kb4`)
  gave about 1 px median vertical residual after rectification. trackingA/B sit 112 mm apart, look 30° outwards and
  25° down. The controller cameras look 21° up.
* Display: Monado direct mode, 3552x3840 per eye, FOV from `lens.conf` (`50 50 44.5 44.5`). There is an unresolved
  display freeze in the compositor present/flip (`analysis/hang-worn-debug.md`).

## 2. Options

| | (a) Compositor underlay in Monado | (b) Rust OpenXR passthrough client (**chosen**) |
|---|---|---|
| What exists in our tree | The `XR_FB_passthrough` state-tracker plumbing exists (`oxr_passthrough.c`, `XRT_LAYER_PASSTHROUGH`, `create_passthrough*`), and IPC forwards the layer, but `XRT_FEATURE_OPENXR_LAYER_FB_PASSTHROUGH=OFF` in our build. **Nothing renders it**: `compositor/multi`, `compositor/main` and `auxiliary/render` have no passthrough code. `XR_HTC_passthrough` is absent | Everything needed: `pimax-cam` (capture, calibration, KB4), the proven `pimax-lenstest` Vulkan upload path, and Monado's timewarp |
| New code | Camera frames into compositor-owned VkImages (driver thread to compositor sync), passthrough layers in the multi compositor, a new layer type in the gfx and compute render paths (shader), blend modes | One Rust crate. No Monado change |
| Risk | Adds GPU work and a new sync point to `comp_renderer`, the code path that freezes today. A compositor bug can wedge the display and need a reboot | A misbehaving client cannot do more than any other client. Quitting it restores the state before |
| IMU rotation compensation | Done in the compositor (fresh pose at render time) | Done **by Monado's existing timewarp**: the client submits each image with the eye pose at camera time (section 4) |
| Latency | Best possible: about one compositor frame after the image is ready | About 0-14 ms more (one app frame) for scene motion. Head rotation is the same as (a) because it is timewarped |
| Under games | Yes (`XR_FB_passthrough` apps, or an always-on underlay) | No: it is its own app. Showing it *over* a game needs `XR_EXTX_overlay` (later) |
| Camera sharing | Driver owns the camera, so Mercury and passthrough can share it | The client opens `/dev/video0`, so it cannot run while the service uses the camera (`PIMAX_HANDS=mercury` / `PIMAX_CAM=1`) |

**Choice: (b) for the first version.** It gives a correct, comfortable, rotation-compensated passthrough today with
zero compositor risk while the display freeze is open. Head rotation stays smooth because the compensation runs where it
is cheapest and latest, in the existing timewarp. All the geometry (maps, remap) is plain Rust with no OpenXR in it, so
option (a) can reuse it later behind a C ABI: `pimax_passthrough_build_lut()` / `pimax_passthrough_remap()` in the
pimax-capi pattern, with thin C glue that feeds a compositor layer. That move only makes sense once the freeze is
understood.

## 3. Geometry

Frames: **body** = the calibration's IMU frame with OpenXR axes (x right, y up, z back; see `pimax_cam::calib`).
**head** = Monado's VIEW space: the same axes, with its origin at the eye centre `E`. **eye** = the OpenXR view.
**camera i** = OpenCV axes of the raw tile (x right, y down, z forward).

* Extrinsics: `body_from_cam_i = (R(ombc), tbc) ∘ cam_i_from_cam0⁻¹` (rig convention `X_i = R X_0 + t`, verified),
  and `head_from_cam_i = T(−E) ∘ body_from_cam_i`. The upside-down mounting is part of this rotation, so no image is
  ever flipped. `E` is not in the calibration. The default is (49.5, 4.5, 30) mm: midway between the left and right
  cameras, midway between the lower and upper cameras, and 5 cm behind the camera plate. Override it with
  `PIMAX_PT_HEAD_OFFSET` / `--head-offset`. It only matters for near objects (2 cm ≈ 0.6° at 2 m).
* Eye ray of pixel (x, y), with OpenXR FOV angles `l < 0 < r` and `d < 0 < u`:
  `d_eye = normalize( tan l + (x+½)/W·(tan r − tan l),  tan u − (y+½)/H·(tan u − tan d),  −1 )`.
* **Fixed-depth reprojection:** `P_head = head_from_eye · (D·d_eye)`, the point at distance D along the eye ray.
  Then `X_cam = cam_from_head · P_head`. For `D = ∞` only the rotations are used: `X_cam = R_cam_from_head R_head_from_eye d_eye`.
* **Fisheye (KB4):** `θ = atan2(√(x²+y²), z)`, `θ_d = θ(1 + k1θ² + k2θ⁴ + k3θ⁶ + k4θ⁸)`, `u = fx θ_d x/r + cx`,
  `v = fy θ_d y/r + cy` (`pimax_cam::kb4`). The map stores u, v in 1/64 px. The per-frame lookup is bilinear.
* **Camera per pixel:** the camera nearest the eye comes first: left eye trackingA (0), then trackingB, then
  controllerA and controllerB. Right eye mirrored. A camera's weight is 1 inside its usable image and falls to 0
  over 24 px towards the tile border or the vignetting circle (r = 330 px). Sample A is the first camera with weight
  `wa > 0`. If `wa < 1`, sample B is the other camera with the largest weight `wb`, and the two blend with
  `α = wa / (wa + (1−wa)·wb)`, which is continuous at every edge. Coverage of the default FOV: left eye 75.7 %
  trackingA, 11.4 % trackingB, 12.9 % controllerA (the top of the view), 0 % uncovered.
* **Errors of a fixed depth.** Camera c sits `b_c` sideways from the eye (trackingA is 24 mm outboard of the left
  eye). A point at distance Z appears shifted by about `b_c·(1/Z − 1/D)` rad. Because each eye uses its own outboard
  camera, the stereo disparity is that of a **~111 mm baseline instead of 63 mm**: objects at distance D are placed
  correctly, nearer ones look nearer than they are (hyperstereo). Vertical offsets between camera and eye (about 20 mm)
  shift near objects vertically. These are the known limits of the "fixed-depth plane" first step, the same as early
  Quest passthrough. The depth key moves the correct distance (default 2 m; this morning's room fits 3 m best, see 5).
* The maps depend only on head-fixed geometry, so they are built once: 28 ms for both eyes at 888x960, 47 ms at the
  live 1184x1280, on a helper thread. They are rebuilt only when the depth or the runtime's views change.

## 4. IMU rotation compensation and timing

The head turns between exposure (t_cam) and display (t_disp). The client renders each image in the eye frame and
submits it as a projection layer **in LOCAL space with the eye poses located at t_cam**
(`xrLocateViews(LOCAL, t_cam)`; Monado answers from the HMD's IMU pose history, `m_relation_history`). The compositor's
timewarp (`render_calc_time_warp_matrix`, on by default) reprojects every layer from its submitted pose to the
display-time pose: `H = R_eye(t_disp)⁻¹ · R_eye(t_cam)`. For `D = ∞` this is exact. For a finite D with 3DoF tracking
the only extra error is the eye's own small translation when the head rotates about its origin, which is negligible.
Each image is resubmitted at the display rate until the next one comes (25 Hz). Head turns are therefore warped at
display rate and look smooth; only moving objects update at 25 Hz.

Camera time to XrTime: `pimax_cam` maps the MCU µs stamp to `CLOCK_MONOTONIC` with its minimum-delay filter (the
client does not have the driver's IMU clock reference). `pose time = mapped − delay`, default **4 ms** (bounds and
reasoning in `timing.rs`; adjust live with `[` `]`). `XR_KHR_convert_timespec_time` converts that time to XrTime.
Without that extension the client uses the display time, which means no compensation, and says so in its log.

**Expected latency** (motion-to-photon for scene content; head rotation is only the compositor part):

| Stage | Time |
|---|---|
| Exposure midpoint → MCU stamp → USB transfer done | ~10-15 ms (Windows: URB completion 11.8 ms after the stamp) |
| Remap both eyes (1184x1280, all cores) | 2.4 ms measured offline |
| Wait for the next app frame, expand + upload (2 × 6 MB staging copy) | 0-14 ms + ~2 ms |
| Compositor + scanout (timewarp to the display pose here) | ~1-2 display frames, 11-25 ms |
| **Total for moving objects** | **~30-55 ms**, updated at 25 Hz |
| **Head rotation** | world-stable up to the compositor's own prediction error, the same as any app |

The client prints these as measured numbers: `capture->ready` and `capture->display` in its status line every 5 s.

## 5. Offline proof (recorded frames, no hardware)

`analysis/passthrough/` was made with `pimax-passthrough render` from the morning2 tiles (lit room, headset still,
08:08), at 888x960 per eye, pinhole images before lens distortion:

* `morning2_depth{2,3,inf}/sbs.png`: left and right eye side by side. `left.png`, `right.png`.
* `coverage_{left,right}.png`: which camera feeds each pixel. Red trackingA, blue trackingB, yellow controllerA,
  green controllerB, blended colours where they cross-fade.
* `overlap_left_cam0_red_cam1_cyan.png`: the left eye rendered only from trackingA (red) and only from trackingB
  (cyan). Grey means they agree.
* `report.txt`: geometry, map statistics, timings and the depth sweep below. `evening2_depth2/` comes from the raw
  `.pxcam` fixture through the full capture parse path (dark room, near objects).

**Real-image geometry check** (left eye, trackingA alone vs trackingB alone, high-pass normalised cross-correlation;
shifts in eye pixels at 0.154°/px):

| depth | 0.75 | 1 | 1.5 | 2 | **3** | 5 | 10 | ∞ |
|---|---|---|---|---|---|---|---|---|
| best dx | +32 | +32 | +16 | +8 | **0** | −4 | −10 | −14 |
| best dy | −2 | −6 | −4 | −2 | **−4** | −4 | −4 | −4 |
| NCC at the best shift | 0.21 | 0.33 | 0.35 | 0.40 | **0.56** | 0.50 | 0.40 | 0.39 |

(+32 is the edge of the search window.)

The horizontal mismatch changes sign monotonically through 0 at 3 m: that is the parallax of a 112 mm camera pair
looking at a room whose main structure (window wall) is ~3 m away. The vertical mismatch is a constant ~4 px ≈ 0.6°
at every depth, a small residual rotation between the two cameras that agrees with the morning rectification. So the
extrinsics, the KB4 model, the eye geometry and the camera choice are right. The pictures also show the limits: the
fixed exposure blows out a sunny room (no exposure control without HID `0x51`), and seams appear where cameras meet on
near objects (fixed depth).

Tests: `cargo test -p pimax-passthrough` gives 28 passed (25 unit, 3 integration). They cover: map vs direct
projection of world points at 0.5/2/∞ m (< 0.02 px), coverage and the mirror symmetry of the eyes, blend weights,
bilinear/blend arithmetic, PNG decode, `.pxcam` replay to tracking frames, **golden eye images** from the real tiles
(`tests/fixtures/golden_*.png`, re-bless with `PIMAX_PT_BLESS=1`), and the real-image overlap check above as a
regression test. Also the camera thread on a `.pxcam` replay (publishes remapped images), pose-time clamping, XR pose/FOV conversion, the map-rebuild key and swapchain format choice.
`cargo clippy -p pimax-passthrough --all-targets -- -D warnings` is clean. The OpenXR/Vulkan part compiles and reuses
lenstest's upload code, but it **has not run against a runtime**: that is the live test.

## 6. Implementation map

| File | Content |
|---|---|
| `rust/pimax-passthrough/src/rig.rs` | calibration → `head_from_cam`, eye-centre offset, per-eye camera order |
| `view.rs` | eye views (pose, FOV, size), pixel ↔ ray |
| `lut.rs` | per-eye map (12-byte taps, 2 samples + weight), multi-threaded build, coverage picture |
| `remap.rs` | bilinear remap, tone curve, grey → RGBA |
| `timing.rs` | camera time → pose time, latency stats |
| `frames.rs` | tracking-frame tiles from `pimax-cam` (live or `.pxcam`) or PNGs |
| `check.rs` | single-camera overlap check and depth sweep |
| `xr.rs` | the OpenXR client (camera thread, map thread, LOCAL-space projection layer at camera-time poses) |
| `main.rs` | CLI `render` / `run` / `rig` |

## 7. First live test (main session)

Preconditions: the display freeze is under control well enough that a plain client (xr_room) runs, so use the
current recommended env for worn sessions. The service must **not** use the camera (no `PIMAX_HANDS=mercury`, no
`PIMAX_CAM=1`), which is the default. Build (already built, offline):
`cd ~/src/pimax-re-passthrough/rust && CARGO_TARGET_DIR=~/src/target-passthrough cargo build --release -p pimax-passthrough`.

```bash
P=~/src/target-passthrough/release/pimax-passthrough
export XR_RUNTIME_JSON=$HOME/src/monado/build/openxr_monado-dev.json IPC_IGNORE_VERSION=1

# 0. service as usual (headset on the desk until the gyro is calibrated)
pimax vr start

# 1. client path without the camera: replay of the lit evening clip (does not touch /dev/video0)
$P run --source ~/src/cam-data/evening-20261013-1959.pxcam --seconds 60
#    expect: "runtime: Monado ... XR_KHR_convert_timespec_time yes", "maps NN ms ...", session FOCUSED,
#    status lines every 5 s with "camera images ~30/s" (that clip is 3/5 tracking frames).
#    In the headset: the recorded desk scene in grey, upright, filling the view. It is a recording, so it
#    re-centres on your gaze with every new image; turning the head must be smooth, with no tearing or judder.

# 2. live cameras, headset on the desk, then worn
$P run
#    expect: "camera: /dev/video0 ...", "camera images 25.0/s", "capture->display" about 30-55 ms.
#    In the headset: the real room in grey, upright, left eye from the left camera, right eye from the right one.
#    Keys in the terminal: p or space = passthrough on/off (off = black), + / - = depth (0.5 m .. inf),
#    [ / ] = camera delay -/+ 2 ms, g = gamma (1.0, 1.5, 2.2, 0.7; 1.5-2.2 helps in a bright room), f = freeze, s = status, q = quit
$P run --off      # starts hidden, p shows it

# 3. stop
#    q in the client terminal, then
pimax vr stop --grace 30
```

What to check and report:
1. **Upright and in the right eye**: nothing mirrored or upside down, the left eye's image comes from the left.
2. **World stays still while turning the head** (the main check of the timewarp path). If the image swims *with*
   the head turn, press `]` (more delay). If it swims *against* it, press `[`. Report the delay that looks still.
3. **Depth**: look at something 2-3 m away, then a hand at 40 cm. The hand looks doubled or too near, as expected
   (section 3). Find the depth that is comfortable for the room (`+` / `-`). Report it.
4. Numbers from the status line: images/s, remap ms, capture->display.
5. Seams: where trackingA meets controllerA (upper part of the view) and in the middle above the nose.
6. If the image is black: the status line says "camera images 0/s" or "no tracking frame for 2 s", meaning another
   process has the camera or the stream is not running (`pimax-cam stats` with the lock, as in cam.md).

Rollback: quit the client. Nothing in Monado or in the service config changed.

## 8. Next steps

1. Live tune: delay and default depth from the test above, then set them as defaults.
2. Exposure control (`0x51`, the Rift S `u_autoexpgain` pattern) once its value ranges are mapped. A lit room is
   overexposed now.
3. Better depth: per-region depth from the trackingA/B disparity (the matcher in `pimax-cam rectify` / `check.rs`)
   instead of one global D; at least auto-pick D with the sweep at start-up.
4. Option (a) once the display freeze is understood: the same maps behind a C ABI in `drv_pimax` (camera already
   there for Mercury), a compositor underlay with `XR_FB_passthrough`, and sharing the camera with Mercury/SLAM.
   Until then, an `XR_EXTX_overlay` mode of this client could put passthrough over a running app (for example, a
   button to peek).
5. GPU remap (the map as an RG16 texture plus one fragment shader) if the CPU expand/upload ever shows up in the
   frame time. It is ~5 ms per image at 25 Hz today, off the compositor.
