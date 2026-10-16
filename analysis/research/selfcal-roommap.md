# Remember the room: a persistent room map and camera self-calibration for the Dream Air on Linux

Date: 2026-10-14. Branch `selfcal` (worktree `~/omarchyvr`). Offline work only: nothing here opened the
headset, the camera, HID or monado-service. Page with pictures and charts: `analysis/selfcal.html`.

The user's idea: *"remember the room: save a 3D map of the room with spot points, and calibrate ourselves with the
cameras through that, no more calibration problems"*. This document checks the idea against what the cameras
actually show, what other headsets and open-source trackers do, and proposes a design. A working prototype of the
calibration half is in `rust/pimax-selfcal`.

## 0. Short answer

* **The idea is sound and is what the big headsets do**, in two separate parts: (1) a tracker that keeps its own
  calibration right while it runs, and (2) a saved map of each room that the headset recognises again. Meta's
  research glasses publish a per-frame online calibration from their SLAM; Quest, Vision Pro, HoloLens and others
  keep room maps and anchors on the device and let the user delete them (section 3).
* **What our cameras show today.** The factory calibration of the two lower ("tracking") cameras is good to
  about 0.2-0.6 degrees. The two upper ("controller") cameras disagree with the lower ones by 3-15 pixels in the
  rectified pairs, which is several degrees if it is real; the recorded frames are too overexposed and too
  close-range to settle it (section 1).
* **Prototype numbers.** On synthetic frames with known errors (0.39-0.56 degrees per camera) the prototype finds
  all three camera rotations to within 0.006 degrees and removes the vertical disparity (median 1.57 px to
  0.03 px). On the one usable real scene it reduces trackingA/B's vertical disparity (median |dy| 0.73 to 0.50 px,
  within 1 px 65 % to 90 %) and the vertical mismatch between the two cameras in the passthrough eye view
  (0.57 to 0.22 degrees). Its own sanity gate refuses to install that result: one scene does not determine the
  rotation well enough (uncertainty 0.2 degrees) and does not generalise across the image (cross-validation flat).
* **Recommended path.** (1) Record one good room sequence (section 7) and run the prototype on it. (2) Bring up
  6DoF with Monado's Basalt (BSD-3) on all four cameras plus the IMU. (3) Add a Rust "calibration monitor" next to
  the tracker that keeps refining the camera rotations from keyframes and writes the overlay between sessions.
  (4) Add the room map (keyframes, 3D points, place recognition, a stored room frame) in Rust; it gives
  relocalisation, persistent boundaries/anchors and, over many sessions, the data for the best calibration.
  Everything stays on the machine, in two places the user can delete.
* **Not a fix for the double red crosshair.** That is display/eye alignment (lens centres versus IPD); the
  tracking cameras cannot see it. Section 2 says what can.

## 1. What the recorded frames show

Data: four tile sets (`analysis/overnight/cam/morning`, `morning2`, `~/src/cam-data/tiles-20261014/ctr38926`,
`ctr70028`) and the three tracking frames of `evening-20261013-1959.pxcam`. The 20 s night clip is black (noise
only). Only `morning2` (the room, lit, headset still on the desk) has enough texture in both lower cameras; the
others are overexposed or show surfaces a few centimetres away.

| Check | Result |
|---|---|
| trackingA/B (pair 0-1), rectified, re-matched | 75-80 matches, median dy -0.67 px (0.17 deg), 65 % within 1 px |
| A/B in the passthrough eye view (camera 0 only vs camera 1 only, sub-pixel correlation) | vertical mismatch -0.57 deg (left eye), -0.55 deg (right) |
| What explains pair 0-1 | a rotation fits (0.51 px left); rotation plus baseline fits a bit better but only with an impossible baseline error (7-9 deg): the matches sit in two clusters (room at ~2 m, desk at ~0.4 m), too little depth and position variety to tell the two apart |
| Controller pairs (0-2, 1-3), search window +-24 px | 0-2: +3 px (27 matches); 1-3: -3 to -15 px depending on the frame, about -8 px overall (54-65 matches over five frames); a single rotation of camera 3 (~4 deg) explains most of 1-3 but leaves 1.7-2 px and the frames disagree |
| Lens image circle vs factory principal point | trackingA: centre within (+3.7, +1.0) px (four corners lit); the others (three corners) within 4 px horizontally: the tiles are cut where the calibration expects, so a shifted readout window is ruled out |
| Earlier "dy about -3.4 px" on pair 1-3 | an artefact of the +-8 px search window, which cut the distribution off |

The upper cameras need a proper recording before anyone believes a several-degree correction (they look up into
bright windows and ceiling here, with the fixed exposure). The tool refuses such pairs on its own (they are "not
used", section 6) and never writes a correction above 2 degrees without `--force`.

## 2. The double red crosshair is not a camera problem

In the lens test each eye sees its own line and the two lines sit side by side. That is the eyes' view of the two
displays: the lens optical centres, the rendered image centres (lens.conf `center`), and the user's eye
positions (IPD) do not agree. The physical lens spacing is set by the IPD motor; command `0x5c`
(`58 06 5c 05 01 <IPD*100 as u16 LE>`) is sent only by Pimax Play on Windows, and the headset reports the live
value in the IMU report (bytes 45-46, IPD*100; 65.88 mm read on Linux on 2026-10-13). Linux never sends it, so the
motor stays where Windows left it.

* **Cameras and a room map cannot measure this.** They look outwards; they see neither the displays nor the eyes.
  A better camera calibration does not move a single display pixel.
* **The eye tracker can, in principle.** Headsets that set IPD automatically measure the 3-D position of each
  eye with their eye trackers: Varjo moves the lenses to the measured IPD ("can automatically adjust the position
  of the lenses based on your interpupillary distance"), HoloLens 2 has no IPD setting at all ("eye positions are
  computed by the system"), Vision Pro aligns its displays at setup, the Vive XR Elite adjusts IPD through eye
  tracking, and Pimax added auto-IPD with its Tobii tracker on the Crystal (press report; the Dream Air FAQ lists
  an auto-IPD range). The usual
  computation is the distance between the two eye (pupil) positions while the user looks at a far target; a Tobii
  patent describes exactly that. Sources in section 9.
* **On this headset** the Tobii module's gaze origin per eye (mm, headset frame) is decoded structurally, but the
  module is blocked by its challenge/response authentication on Linux. Caveat found in the notes: the host also
  sends the IPD to the module (`0x5be` = +-IPD/2), so an IPD derived from the reported eye origins could be an
  echo of the host's value; that must be tested before trusting it.
* **Practical path now:** read and log the live motor IPD; make the per-eye render centre follow the physical lens
  spacing; let the user set the motor from Linux (a small `0x5c` tool, the motor moves ~1.9 mm/s); measure the
  user's IPD once by other means; use the eye tracker when it is unblocked.

## 3. What other headsets do

Sources and confidence tags in section 9 (CONFIRMED = read on the vendor's own page; SECONDARY = press, forum,
search snippet; NOT FOUND = searched, no source). "Online calibration" means the tracker re-estimating camera or
IMU calibration while in use; a user-facing recalibration menu is listed separately.

| Headset | Online / self-calibration | Persistent room map | Where stored, how to delete | IPD |
|---|---|---|---|---|
| Meta Quest (Insight) | Not stated for Quest (NOT FOUND); the Insight blog only says landmarks let it "compensate for drift". Meta's Project Aria SLAM output publishes an **online calibration per frame**: intrinsics, extrinsics and time offsets of every sensor (CONFIRMED) | Yes: Insight keeps "a 3D map that is generated and constantly updated"; the device "can recall previously set up environments"; up to ~15 rooms on Quest 3 (SECONDARY); spatial anchors local or shared via the cloud with a TTL (CONFIRMED) | On device; "Enhanced spatial services" uploads spatial data to Meta (opt-in). Delete: Settings > Privacy and safety > Device permissions > Clear physical space history / Delete spatial data from Meta servers (CONFIRMED) | Manual (3 presets on Quest 2, wheel on Quest 3); no eye-tracked fit found for Quest Pro |
| Apple Vision Pro | NOT FOUND (no public statement) | World anchors persist across app runs ("ARKit persists world anchor UUIDs and transforms across multiple runs"); room tracking provider (CONFIRMED); number of rooms NOT FOUND | On device "whenever possible" (SECONDARY); reset: Settings > General > Transfer or Reset > Reset Surroundings Data (SECONDARY) | Automatic display alignment at setup (press and hold the Digital Crown) (CONFIRMED) |
| Windows MR / HoloLens | Not documented as camera calibration. Archived WMR FAQ: sensor errors are recorded "into the environment data on your PC" (https://learn.microsoft.com/en-us/previous-versions/mixed-reality/enthusiast-guide/tracking). HoloLens 2 only "tune[s] its visuals" for eye-position shifts (https://learn.microsoft.com/en-us/hololens/hololens-calibration, CONFIRMED) | HoloLens: "builds up a spatial map of the environment", relocalised against map sections "stored on the device" (https://learn.microsoft.com/en-us/hololens/hololens-spaces, CONFIRMED). WMR: environment data on the PC (archived FAQ); WMR is deprecated (https://learn.microsoft.com/en-my/hololens/hololens2-support) | HoloLens: map database not user-accessible; "Remove all holograms" clears it (same spaces page). WMR: "Clear Environment Data" (forum, SECONDARY) | HoloLens 2: no IPD setting, "eye positions are computed by the system" (CONFIRMED) |
| Pico 4 / Enterprise | Not documented publicly. The PICO Unity SDK docs index lists no anchor, boundary or calibration pages (https://developer.picoxr.com/document/unity/) | Not documented publicly; no spatial-anchor or boundary-storage page found | Not documented publicly | Pico 4 Enterprise: "automatically detect new headset users and start the inter-pupillary distance setup" (ArborXR MDM guide, SECONDARY: https://help.arborxr.com/en/articles/6630022-configure-pico-system-settings) |
| HTC Vive Focus 3 / XR Elite / Focus Vision | Not documented publicly. The official Focus Vision tracking FAQ covers room setup and resets only, not calibration (https://www.vive.com/us/support/focusvision/faq/how-to-optimize-tracking-in-my-space.html, CONFIRMED) | Room setup must "look in all directions to establish tracking for the whole room"; rerun via Settings > Boundary > Reset next to Safety boundary (same FAQ). Stored form not described | Not documented publicly (the Reset step does not say what it deletes) | XR Elite: "automatically adjust IPD via eye tracking" (CONFIRMED); Focus Vision: auto IPD (SECONDARY) |
| Varjo XR-3/XR-4 | XR-4: "Varjo inside-out tracking will automatically calibrate the positional tracking cameras in the background while in use", on by default (https://support.varjo.com/hc/en-us/automatic-tracking-camera-calibration-xr-4, CONFIRMED). IMU and intrinsics not named. XR-3: manual calibration run (https://support.varjo.com/hc/en-us/room-setup-varjo-inside-out-tracking) | Room setup sets floor level and forward direction; the headset "can remember where it was last used" and does not start room setup automatically (same room-setup page, CONFIRMED) | Camera calibration data is saved: turning auto-calibration off uses "the latest saved calibration data", turning it on "will delete all previous calibration data" (XR-4 page). Room data location not documented | Automatic lens IPD from eye tracking, on by default (https://support.varjo.com/hc/en-us/adjusting-the-headset-lenses, CONFIRMED) |
| Valve (Index, Steam Frame) | Lighthouse base-station re-solve: not documented publicly (Valve's tracking partner page describes no base-station calibration: https://partner.steamgames.com/vrtracking, CONFIRMED). Steam Frame: only "inside-out camera-based tracking" (vr.org, SECONDARY: https://vr.org/steam-frame-specs) | SteamVR play area in a chaperone file with per-base-station "universes" (Valve wiki snippet, SECONDARY: https://developer.valvesoftware.com/wiki/SteamVR/chaperone_info.vrchap). Steam Frame: not documented publicly | Local files. Community fix after a base-station change: delete stored lighthouse data and chaperone file, then recalibrate (vronlinux.org, SECONDARY) | Index: manual slider, 58-70 mm (UploadVR hands-on, SECONDARY: https://uploadvr.com/valve-index-hands-on/). Steam Frame: manual 60-70 mm (vr.org, SECONDARY) |

The pattern that matters for us: **calibration correction runs inside or next to the tracker, maps and anchors stay
on the device by default, cloud sharing is opt-in, and every vendor offers a delete button**. Nobody publishes a
user-facing "recalibrate the cameras" procedure for inside-out headsets; when calibration is visibly off, the
tracker absorbs it or the device goes back to the factory.

## 4. Open-source building blocks

| System | Licence | Online calibration | Map save / load, relocalisation | Fit for us |
|---|---|---|---|---|
| **Basalt** (upstream; Monado's fork is Monado's default SLAM) | BSD-3-Clause | None: VIO state is pose, velocity, IMU biases. Offline tools calibrate cameras, camera-IMU and time offset | `basalt_mapper` builds a global map offline from VIO "non-linear factors"; no map save/load or relocalisation found | Best start: permissive, already wired in Monado via VIT, the fork runs more than two cameras, has KB4 |
| **Monado VIT + t_slam** | BSL-1.0 | VIT 2.0.1 takes camera/IMU calibration only **before** start; no call returns a refined calibration | no map save/load call (only an implementation-specific config file); `vit_tracker_reset` exists | the plumbing we feed; extend with two extensions (refined calibration out, map load/save) in the long run |
| **OpenVINS** | GPL-3.0 | Yes, online: camera-IMU extrinsics, camera intrinsics, time offset (`calib_cam_extrinsics`, `calib_cam_intrinsics`, `calib_cam_timeoffset`) | loop closure in a separate module (ov_secondary), multi-session merge via ov_maplab | reference for what to estimate online; GPL |
| **VINS-Fusion** | GPL-3.0 | Yes: camera-IMU extrinsics and time offset | pose graph save/load (`pose_graph_save_path`, `load_previous_pose_graph`), DBoW2 loop closure | reference; GPL, ROS, two cameras |
| **ORB-SLAM3** (Atlas) | GPL-3.0 | No | multi-map Atlas, map merging, relocalisation, Atlas save/load | best-known map persistence; GPL |
| **maplab** (+ ROVIO / ROVIOLI) | Apache-2.0 (ROVIO BSD-3) | ROVIO estimates camera-IMU extrinsics "during runtime"; maplab's batch step "can jointly estimate the intrinsics and extrinsics" | map `save` / `load`, multi-session co-registration, ROVIOLI localises against a saved map | the closest design reference for multi-session maps with calibration refinement; ROS-heavy |
| **Kimera** | BSD-2-Clause | No | loop closure (off by default), mesh; no map save/load found | pinhole/omni cameras only; not needed |
| **Kalibr** | BSD-4-Clause (advertising clause) | offline multi-camera + camera-IMU spatial/temporal calibration with a target; `equi` = KB4 | n/a | for a one-off lab calibration, not online |
| **hloc / COLMAP** | Apache-2.0 / BSD-3 (SuperPoint and SuperGlue weights are non-commercial; LightGlue Apache-2.0) | COLMAP: rig poses from registered images, self-calibrating BA offline | register new images against a prebuilt model | offline tools; too heavy for runtime |

**Licences and Monado.** Monado is BSL-1.0, which is listed as GPL-compatible (Wikipedia, citing the FSF list; the
FSF pages themselves could not be fetched). A GPL tracker can be combined with it, but a combined binary that is
*distributed* must then follow the GPL as a whole; for a personal build that is not distributed, nothing changes.
Monado keeps SLAM systems out of its tree: in 2022 each system implemented a `slam_tracker.hpp` header in its own
fork (Kimera-VIO, ORB-SLAM3 and Basalt were integrated that way); the current checkout loads the system at run time
through VIT (`dlopen`, `VIT_SYSTEM_LIBRARY_PATH`, default `libbasalt.so`). Whether a `dlopen`ed GPL library makes one
combined work is the GPL FAQ's plug-in question (entries `#GPLPluginsInNF`, `#MereAggregation`); the safe reading
is "yes". Recommendation: keep our own code MIT/Apache in Rust, use Basalt (BSD-3), use the GPL systems as
references only.

## 5. Design

### 5.1 Online self-calibration inside 6DoF tracking

What to estimate, in the order it pays off:

1. **Camera-to-camera rotations** (3 numbers per camera besides the reference). The dominant error seen here,
   observable from stereo matches alone, no motion needed. *Prototype done* (`pimax-selfcal`).
2. **Camera-IMU rotation and time offset.** Needed for good 6DoF; only observable with motion. Basalt does not
   estimate them online; a cheap, robust estimator aligns the gyro's angular velocity with the camera rotation
   between frames (hand-eye on rotations), plus the time shift that maximises their correlation. The factory IMU
   numbers are already known to be off (the XML gyro bias has the wrong sign on z, `analysis/imu-orientation.md`).
3. **Camera translations** (millimetres). Weakly observable, needs near points at several depths; diagnose
   (the prototype's rotation/baseline test), estimate only with tight priors and lots of data.
4. **Intrinsics.** Keep the factory values; the lens-circle check is the sanity test.

Where it runs. VIT accepts the calibration only before the tracker starts and cannot hand one back, so the
estimator runs **next to** the tracker, not inside Basalt:

* the driver already has every tracking frame (25 Hz, four tiles) and the 1 kHz IMU on one clock;
* a *calibration monitor* thread takes at most ~1 frame per second, keeps frames that are well exposed (mean
  40-200, under 5 % clipped), textured and new (orientation buckets, depth spread), matches them and keeps only the
  matches (not images) in a rolling buffer;
* at the end of a session (or every few minutes on idle) it runs the bundle adjustment with the current overlay as
  prior and checks the gate;
* a new overlay is written only if the gate passes **and** the change is significant (more than 3 sigma and
  0.05 deg) **and** two independent batches agree (two sessions, or two halves of one). The tracker picks it up at
  its next start.

Sequential refinement. Each estimate starts from the installed overlay as prior, widened by a random walk
(0.02 deg per square-root day; `pimax-selfcal estimate --from default` does this): small consistent evidence
accumulates, a single bad session cannot move the calibration far.

Camera-IMU rotation and time offset (step 2 above), the simple robust way: the camera's angular velocity between
frames (from the tracker's poses, or from the frame-to-frame rotation of matched features) must equal the gyro's,
rotated and delayed: `w_cam(t) = R_cam_imu * w_imu(t + dt)`. First `dt` from the peak of the correlation of the
two rates' magnitudes (rotation does not change a magnitude), then `R_cam_imu` from all pairs of rate vectors by
an SVD (Kabsch), then both refined together. It needs rotation about at least two axes (the recording in
section 7 pans, tilts and rolls). The factory XML's camera-IMU time offset (`delta`, 1.638 ms) is the starting
value; the drivers do not apply it today.

Sanity bounds (the prototype's gate): correction at most 2 deg per camera (3 deg hard limit when loading),
one-sigma uncertainty at most 0.05 deg per axis, at least 100 inlier observations per camera, bundle-adjustment
residual at most 0.8 px, no pair with an ambiguous or baseline-type diagnosis, re-matched pairs and held-out image
blocks must improve. **Rollback**: every save keeps the previous overlay as `.prev` (`pimax-selfcal rollback`
swaps them, `clear` returns to the factory calibration). Later, an automatic rollback when tracking quality drops
after a new overlay (more relocalisations, larger reprojection error).

Storage: `~/.config/omarchyvr/cam-calib-overlay.json`, next to the factory XML and never inside it:

```json
{
  "format": "pimax-cam-calib-overlay", "version": 1,
  "device_uid": "P511X0X000000000001", "factory_calibration_date": "2026_07_28_13_05_49",
  "created_unix": 1791944193, "tool": "pimax-selfcal 0.1.0", "reference_camera": 0,
  "convention": "camera rotated about its own centre: cam_from_X' = Exp(rotation_vector) * cam_from_X; radians, OpenCV camera axes; rig X_i = R X_0 + t",
  "cameras": [ { "id": 1, "rotation_vector": [0.0014, -0.0095, 0.0038], "angle_deg": 0.594,
                 "sigma": [0.00095, 0.0036, 0.0019] } ],
  "quality": { "matches_before": 80, "median_abs_dy_before_px": 0.805, "median_abs_dy_after_px": 0.573, "...": 0 },
  "note": "from 7 scene(s): ..."
}
```

Readers (`pimax-cam`, `omarchyvr-passthrough`) refuse it for another headset (device UID), another factory calibration
(date), unknown camera ids or any rotation above 3 deg, and fall back to the factory values. Version 2 would add
camera-IMU rotation, time offset and translations, each with a sigma; readers ignore fields they do not know.

### 5.2 A persistent map per room

Two jobs: **relocalise** at session start (the same world frame every time, so boundary, anchors and desktop
windows stay where they were), and **accumulate evidence** ("spot points" seen in many sessions) for the
calibration.

Contents of one room (`~/.local/share/omarchyvr/rooms/<uuid>/`), no images:

| File | Content | Size |
|---|---|---|
| `room.json` | name, created/last used, gravity-aligned room frame, floor height, boundary polygon, named anchors, overlay used | KB |
| `keyframes.bin` | per keyframe: body pose in the room frame, and per camera up to 200 features (pixel, 256-bit binary descriptor, landmark id) | ~300 keyframes x 4 x 200 x 44 B = ~10 MB |
| `landmarks.bin` | 3-D points in the room frame, quality, observation count, last seen | ~50 k x 32 B = 1.6 MB |

Recognition: binary descriptors voted against the room's keyframes (a home has a handful of rooms and a few
hundred keyframes each, so brute force or a small bag of words is enough), then a multi-camera PnP with RANSAC
using all four cameras, with gravity from the IMU fixing two of the angles. Accept with at least 50 inliers and
a reprojection error under 2 px; the result is a fixed transform room_from_slam applied on top of the tracker's
poses. Updating: add keyframes from new viewpoints at the end of a session, retire points not seen again for
several sessions (moved furniture), cap the size; an idle-time multi-session bundle adjustment refines landmarks,
keyframes **and the camera rotations** shared by all sessions. That is where "calibrate ourselves through the map"
becomes strong: thousands of points seen from many places, at all depths and image positions, which is exactly
what one scene lacks.

### 5.3 Monado integration

Today: `drv_pimax` is 3DoF (`m_imu_3dof`); the camera code is behind `-DPIMAX_CAM=ON`; tiles 0 and 1 go to Mercury
and to a placeholder sink that only counts frames; no IMU sink is fed. Steps:

1. **6DoF**: `t_slam_create` with four cameras (L8 640x480, same timestamp per bundle, index order) and the IMU
   sink fed from `pimax_on_imu` (IMU-clock ns); `t_slam_calibration` from `pimax_cam_calib` (KB4,
   `T_imu_cam = body_from_cam`, 25 Hz, IMU noise from the measured values); head pose from
   `xrt_tracked_slam_get_tracked_pose` into the relation history, with the 3DoF fusion as fallback (VIT has no
   "tracking lost" flag, so watch for jumps and timeouts).
2. **Calibration monitor and room map in Rust** behind a C ABI in the `pimax-capi` style (a sketch; not built):

```c
typedef struct pimax_selfcal pimax_selfcal;
pimax_selfcal *pimax_selfcal_create(const pimax_cam_calib *factory, const char *overlay_path, const char *rooms_dir);
void pimax_selfcal_destroy(pimax_selfcal *s);
/* one tracking frame: four 640x480 L8 tiles, IMU-clock time; copies into a bounded queue, never blocks */
void pimax_selfcal_push_frame(pimax_selfcal *s, uint64_t time_ns, const uint8_t *const tiles[4], uint32_t stride);
void pimax_selfcal_push_pose(pimax_selfcal *s, uint64_t time_ns, const double pose[7]); /* tracker body pose */
int pimax_selfcal_room_from_tracker(pimax_selfcal *s, double pose[7]);  /* 0 once relocalised in a known room */
int pimax_selfcal_poll(pimax_selfcal *s, pimax_selfcal_status *out);   /* new overlay written, room name, ... */
```

   One low-priority worker thread, preallocated queue, a few percent of one core.
3. **Hand-off**: the driver builds the SLAM and Mercury calibrations from factory + overlay at the next start,
   only when the user opted in (for example `PIMAX_CAM_OVERLAY=1`). Not wired in this branch, on purpose.
4. **Upstream route** (long term): two VIT extensions, "refined calibration out" and "map load/save", implemented
   in the Basalt fork; then the monitor could move into the tracker.

### 5.4 Privacy

* Local only: the overlay (~1 KB of numbers) in `~/.config/omarchyvr/`, room maps in `~/.local/share/omarchyvr/rooms/`,
  created with user-only permissions. None of this code has a network dependency; nothing is uploaded.
* No images are stored in a map, only features and 3-D points. Feature maps can still be turned back into
  recognisable pictures of the room (Pittaluga et al., "Revealing Scenes by Inverting Structure from Motion
  Reconstructions", CVPR 2019), so they are treated as private data like photos.
* Delete: `pimax-selfcal clear` removes the overlay and its backup; deleting a room's directory removes the room
  (planned: `pimax rooms list | forget NAME | forget --all`, and a "forget all spatial data" item in the launcher);
  rooms unused for 90 days are pruned. Debug recordings (`.pxcam`) are raw camera video: they stay outside the
  repository (`~/src/cam-data`) and should be deleted after use.

### 5.5 What it fixes and what it does not

| Fixes | Does not fix |
|---|---|
| Small rotations between cameras (0.2-0.6 deg seen on A/B): better stereo for Mercury hands (a 0.4 deg vergence error is ~3 % depth at 0.5 m) and for SLAM | The double crosshair, lens distortion, panel rotation: display side, invisible to these cameras |
| Vertical disparity between the eyes in the passthrough (0.57 deg measured) | Exposure (needs HID 0x51), the 25 Hz tracking rate, the NVIDIA flip hang |
| (with VIO) camera-IMU rotation and time offset: less drift and jitter | Large mechanical damage: detected and refused, not silently absorbed |
| Same room frame every session: boundary, anchors, window placement persist; faster start | Millimetre camera positions (diagnosed only), intrinsics (sanity check only) |
| Calibration that improves with use instead of ageing | Rooms that change completely; dark rooms (no features) |

## 6. Prototype: `rust/pimax-selfcal`

Pipeline (`pipeline.rs`), all offline on recorded frames:

1. Rectify each camera pair of each frame with the current calibration (`rect.rs`), find corners and match them
   with a band search, a left-right check and Lucas-Kanade sub-pixel refinement that ignores clipped pixels
   (`matcher.rs`). Matches become pairs of unit rays.
2. Diagnose each pair (`diag.rs`): fit the vertical-disparity field with a rotation model, a baseline-direction
   model and both (robust least median of squares); verdict *fine / rotation / rotation+baseline / ambiguous /
   unexplained / too few*. Only usable pairs feed the adjustment, so a pair no rotation explains cannot drag a
   camera it shares with a good pair.
3. Rotation-only bundle adjustment over all usable pairs and frames (`ba.rs`): one rotation per free camera about
   its own centre, one inverse depth per match (Schur complement), Huber, outlier round, prior 0.3 deg per axis,
   covariance. Two rounds (match again with the corrected rectification).
4. Measure before/after by re-rectifying and re-matching (not the model's own residuals), split far/near,
   spatial cross-validation (96 px image blocks held out), leave-one-frame-out, the passthrough eye-view check
   (`eyecheck.rs`: one eye rendered from camera 0 only and camera 1 only, sub-pixel correlation), the gate, and
   the overlay.

Also `circle.rs` (lens image circle vs principal point), `synth.rs` (a textured room rendered pixel by pixel
through any calibration), `viz.rs` (pictures).

| Result | Value |
|---|---|
| Synthetic, 3 rigs poses, cameras 1-3 rotated 0.39 / 0.45 / 0.50 deg (`selftest`) | found to 0.0028 / 0.0045 / 0.0053 deg; median \|dy\| 1.57 -> 0.03 px over 16 k matches; held-out blocks 1.18 -> 0.03 px; eye view -0.51 -> -0.07 deg; gate PASS |
| Synthetic test (`tests/synthetic.rs`), 0.44 / 0.51 / 0.56 deg | all within 0.03 deg (about 0.005), every pair at 0.03-0.04 px after, overlay reproduces the true rig, a second run changes nothing |
| Synthetic camera displaced by 6 mm (no rotation) | diagnosed as a baseline-direction error of 0.054 (6 mm / 112 mm), gate refuses |
| Real, morning2 + 6 other frames | only pair 0-1 usable; camera 1: 0.594 deg (sigma 0.05 / 0.20 / 0.11 deg; head axes pitch -0.21, yaw -0.40, roll +0.38) |
| Real pair 0-1, re-matched | median \|dy\| 0.81 -> 0.57 px over all 80 matches (morning2 alone 0.73 -> 0.50; within 1 px 65 -> 90 %); far 1.00 -> 0.63, near 0.56 -> 0.40 |
| Real, eye view (independent method) | vertical mismatch -0.57 -> -0.22 deg (left eye), -0.55 -> -0.22 deg (right) |
| Real, spatial cross-validation | 0.45 -> 0.48 px (no gain on held-out image blocks) |
| Real, gate | FAIL: uncertainty 0.20 deg > 0.05, 76 observations < 100, rotation vs baseline not separable, cross-validation |
| Prior width | with a loose prior (2 deg) the poorly determined yaw-like part grows to 0.7 deg and the eye-view correlation drops; 0.3 deg keeps it in check |

Commands (binaries in `~/src/target-selfcal/release/`):

```bash
S=~/src/target-selfcal/release/pimax-selfcal
$S selftest --out /tmp/selftest                       # synthetic end-to-end, PASS/FAIL, pictures
$S estimate ~/omarchyvr/analysis/overnight/cam/morning2 --out /tmp/real --holdout   # report, summary.json, pictures
$S estimate FILE.pxcam@0:200:5 --out DIR              # 200 tracking frames, every 5th
$S estimate ... --write                                # install ~/.config/omarchyvr/cam-calib-overlay.json if the gate passes
$S check FILE.pxcam@0:100:10 --overlay                # factory vs factory+overlay, eye-view check
$S pairs SCENES --dy-search 24 --out DIR              # per-pair matches, rejection reasons, pictures, diagnosis
$S lens SCENES                                        # image circle vs principal point
$S show | rollback | clear                            # the installed overlay
P=$CARGO_TARGET_DIR/release/omarchyvr-passthrough  # (or ~/src/target-selfcal/release/)
$P render --tiles PREFIX --out DIR --overlay default  # passthrough with the learned calibration
$P run --overlay default                              # live client, opt-in
~/src/target-selfcal/release/pimax-cam rectify SRC --out DIR --overlay default
```

Tests (`CARGO_TARGET_DIR=~/src/target-selfcal cargo test -p pimax-selfcal -p pimax-cam -p omarchyvr-passthrough`):
pimax-selfcal 25 unit + 2 synthetic end-to-end + 2 real-data tests (about 20 s, most of it the synthetic
rendering and matching); omarchyvr-passthrough 26 unit + 3 integration (new: overlay applied to the rig, sub-pixel
peak); pimax-cam 48 unit + 3 integration (2 live tests ignored). `cargo clippy -p pimax-selfcal -p pimax-cam
-p omarchyvr-passthrough --all-targets -- -D warnings` and `cargo fmt --check` are clean; `cargo check --workspace
--all-targets` passes.

## 7. Recording request (main session)

Goal: frames from which the controller-camera question can be settled and the A/B correction validated. No
service may use the camera while recording (no `PIMAX_HANDS=mercury`, no `PIMAX_CAM=1`).

What the user does (about one minute):

1. Light: room lights on, **no direct sun and no bright window filling the view** (the exposure is fixed at
   10.4 ms / gain 449 and blows out daylight). Evening with lamps, curtains half closed, is ideal.
2. Hold the headset in the hands, cameras facing the room (it does not need to be worn). Keep hands and people out
   of the first 50 cm in front of it.
3. Slowly (about 3 s per position) visit 15-20 views: furniture, shelves, posters, door frames at **1 to 4 m**;
   tilt the headset **up and down by ~30 deg** so that the upper and lower cameras both see the same textured
   wall at eye height; **roll it 30-45 deg** left and right a few times; finish with a few near views (a cluttered
   desk at 0.5-1 m).

Commands (the same lock as before):

```bash
L="flock -o -w 3600 /tmp/omarchyvr-headset.lock"
C=~/src/target-selfcal/release/pimax-cam
$L $C stats --seconds 5                     # expect ~50 fps, lost 0
$L $C tiles "" --out /tmp/selfcal-tiles --kind tracking   # look at the mosaic: not white, not black?
$L $C record ~/src/cam-data/selfcal-room-$(date +%Y%m%d-%H%M).pxcam --seconds 60 --zlib 1
```

Then offline (no headset needed):

```bash
S=~/src/target-selfcal/release/pimax-selfcal; F=~/src/cam-data/selfcal-room-*.pxcam
$S lens $F@0:300:10
$S pairs $F@0:300:20 --dy-search 24 --out ~/src/selfcal-results/room-pairs
$S estimate $F@0:300:5 --out ~/src/selfcal-results/room --holdout
```

Expected: hundreds of matches per pair. If the controller pairs still need several degrees with a clean rotation
fit and a passing cross-validation, the factory extrinsics of the upper cameras are off and the overlay should be
installed (`--write`); if they stay "unexplained", the problem is elsewhere (non-central fisheye at near range,
or the controller cameras' model).

Later, for the room map and VIO: the same walk with the headset worn and the IMU recorded on the same clock
(`pimax-cam record` stores camera frames only; an IMU logger with device timestamps is the missing piece, or
Monado's EuRoC recorder once the SLAM tracker is wired).

## 8. Open questions and risks

* The controller cameras (section 1): real calibration error, or an effect of near-field, overexposed data?
* The weakly determined rotation component (yaw-like, about the vertical axis of the pair) needs matches spread
  over the whole image and at several depths; a still headset on a desk gives neither.
* Partial overlays move other pairs: correcting camera 1 alone made pair 1-3 worse (-7.3 -> -8.2 px). Estimate all
  cameras together once the data allows.
* Fisheye lenses are not central: for points closer than ~0.5 m the entrance pupil shift (a few mm) matters at the
  0.3 deg level, which KB4 cannot express. Rotations should come from far points.
* VIT has no calibration read-back and no map API; until it does, calibration changes apply at tracker restart.

## 9. Sources

Research notes with quotes: `~/src/selfcal-research/` (outside the repository): `vendors-meta-pico-htc.md`,
`vendors-apple-ms-varjo-valve.md`, `oss-basalt-openvins-vins.md`, `oss-orbslam-maplab-licences.md`,
`ipd-eyetracking.md`, `monado-explore.md` (local Monado checkout, with file:line references).

Headsets:
* Meta Insight blog: https://ai.meta.com/blog/powered-by-ai-oculus-insight/ ; environments:
  https://developers.meta.com/horizon/design/head-technology/ ; spatial data settings:
  https://www.meta.com/help/quest/articles/accounts/privacy-information-and-settings/spatial-data-settings/ ;
  boundary/space setup: https://www.meta.com/help/quest/463504908043519/ ; shared anchors:
  https://developers.meta.com/horizon/documentation/unreal/unreal-spatial-anchors-sharing/ ; 15 rooms (SECONDARY):
  https://mixed-news.com/en/meta-quest-3-multi-room-mixed-reality/ ; IPD: https://www.meta.com/help/quest/261777072346131/
* Project Aria online calibration: https://facebookresearch.github.io/projectaria_tools/docs/data_formats/mps/slam/mps_calibration
* Apple: WorldAnchor https://developer.apple.com/tutorials/data/documentation/arkit/worldanchor.json ;
  RoomTrackingProvider https://developer.apple.com/tutorials/data/documentation/arkit/roomtrackingprovider.json ;
  display alignment https://support.apple.com/en-us/120050 ; eyes and hands privacy
  https://www.apple.com/legal/privacy/data/en/eyes-hands/ ; reset surroundings (SECONDARY)
  https://imore.com/vision-pro/apple-vision-pro-essentials-how-to-update-turn-off-or-reset-the-headset
* HoloLens 2 IPD: https://learn.microsoft.com/hololens/hololens-calibration ; Varjo lenses:
  https://support.varjo.com/hc/en-us/adjusting-the-headset-lenses ; Vive XR Elite auto IPD:
  https://business.vive.com/eu/support/vive-xr-face-tracker/category_howto/auto-ipd-adjustment.html ; Pimax auto-IPD
  (SECONDARY): https://uploadvr.com/pimax-crystal-eye-tracking-standalone-mode-update/ ; Tobii IPD patent:
  https://patents.google.com/patent/US20240320853A1/en

Vendor rows re-checked (this update): Microsoft WMR FAQ (archived) https://learn.microsoft.com/en-us/previous-versions/mixed-reality/enthusiast-guide/tracking ;
  Varjo XR-4 auto calibration https://support.varjo.com/hc/en-us/automatic-tracking-camera-calibration-xr-4 ;
  Vive Focus Vision tracking FAQ https://www.vive.com/us/support/focusvision/faq/how-to-optimize-tracking-in-my-space.html ;
  Pico Unity SDK docs https://developer.picoxr.com/document/unity/ ; ArborXR Pico settings (SECONDARY) https://help.arborxr.com/en/articles/6630022-configure-pico-system-settings ;
  Valve tracking partner page https://partner.steamgames.com/vrtracking ; Steam Frame specs (SECONDARY) https://vr.org/steam-frame-specs ;
  UploadVR Index hands-on (SECONDARY) https://uploadvr.com/valve-index-hands-on/

Open source:
* Basalt: https://gitlab.com/VladyslavUsenko/basalt (README, doc/Calibration.md, doc/VioMapping.md); paper
  Usenko et al., "Visual-Inertial Mapping with Non-Linear Factor Recovery", RA-L 2020, https://arxiv.org/abs/1904.06504 ;
  Monado SLAM dataset (de Mayo, Cremers, Pire): https://arxiv.org/abs/2508.00088
* Monado: https://monado.freedesktop.org/ ; VIT loader https://monado.pages.freedesktop.org/monado/t__vit__loader_8h.html ;
  local `~/src/monado/src/external/vit_includes/vit/vit_interface.h` (VIT 2.0.1), `src/xrt/auxiliary/tracking/t_tracker_slam.cpp`
* OpenVINS: https://github.com/rpng/open_vins (config/euroc_mav/estimator_config.yaml); Geneva et al., ICRA 2020
* VINS-Fusion: https://github.com/HKUST-Aerial-Robotics/VINS-Fusion ; Qin & Shen, "Online Temporal Calibration for
  Monocular Visual-Inertial Systems", IROS 2018, https://arxiv.org/abs/1808.00692
* ORB-SLAM3: https://github.com/UZ-SLAMLab/ORB_SLAM3 (Atlas options in src/Settings.cc); Campos et al., T-RO 2021,
  https://arxiv.org/abs/2007.11898
* maplab: https://github.com/ethz-asl/maplab (wiki: Console-map-management, Running-ROVIOLI-in-Localization-mode,
  Sensor-calibration-refinement); ROVIO: https://github.com/ethz-asl/rovio ; maplab 2.0, RA-L 2023,
  https://arxiv.org/abs/2212.00654
* Kimera-VIO: https://github.com/MIT-SPARK/Kimera-VIO ; Kalibr: https://github.com/ethz-asl/kalibr (LICENSE is
  BSD-4-Clause); hloc: https://github.com/cvg/Hierarchical-Localization ; COLMAP: https://github.com/colmap/colmap ,
  https://colmap.github.io/rigs.html
* Stereo self-calibration: Hansen et al., "Online Continuous Stereo Extrinsic Parameter Estimation", CVPR 2012;
  Dang, Hoffmann, Stiller, "Continuous Stereo Self-Calibration by Camera Parameter Tracking", IEEE TIP 2009;
  Ling and Shen, "High-Precision Online Markerless Stereo Extrinsic Calibration", IROS 2016,
  https://arxiv.org/abs/1903.10705
* Licences: https://en.wikipedia.org/wiki/Boost_Software_License (cites the FSF list); GPL FAQ
  https://www.gnu.org/licenses/gpl-faq.html ; Collabora on Monado's SLAM integration (2022):
  https://www.collabora.com/news-and-blog/blog/2022/04/05/visual-inertial-tracking-support-for-monado-openxr/

Local evidence: `analysis/overnight/cam.md`, `analysis/passthrough-design.md`, `analysis/imu-orientation.md`,
`analysis/controllers-settings.md` (IPD command and live value), results of this work in
`analysis/selfcal/` (reports, summaries, pictures).
