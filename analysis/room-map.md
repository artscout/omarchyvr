# Persistent room map: save the room, recognise it next start (2026-10-14)

Goal: the world origin of 6DoF tracking (and with it every WayVR window, controller and gaze ray) comes back to
the same real place each session, **without a printed tag**. Windows Pimax does this with an ORB map of the room
plus a DBoW vocabulary (`platMap_orborb_K8L4.bin`, `voc_GEN_8X4.dbow`: "load map to reloc, and save map").
Code: `rust/omarchyvr-roommap` (on top of `rust/omarchyvr-room`). Everything here ran offline on recordings. Nothing
touched Monado, the broker or the headset.

## 1. Options

| Option | Verdict |
|---|---|
| **Basalt map save/load** | **Not available.** Our Basalt is the Monado VIT fork (`~/src/basalt`, mateosss df6e970, VIT 2.0.1). It is a sliding-window VIO. The only map-related pieces are `--marg-data` (dumps marginalisation data) and the offline `basalt_mapper` (`NfrMapper`: non-linear factor recovery + `HashBow` loop closure, Pangolin GUI). Nothing loads a map into the running estimator or relocalises against one, and VIT has no API for it. Adding it means deep C++ changes in Basalt, VIT and Monado's `t_tracker_slam`. Rejected. |
| **ORB-SLAM3 Atlas** (save/load, DBoW2 reloc) | Does exactly this, but it is GPLv3 and would be a second full SLAM next to Basalt. Reference only. |
| **Our own keyframe + ORB place-recognition database, PnP against saved 3D points, result written to the `PIMAX_ANCHOR` offset hook** | **Chosen.** Runs beside Basalt, read-only on the camera broker and the Monado head ring. The Monado side already exists: the AprilTag anchor's offset file (`app_from_slam = offset * slam`). Monado itself needs no change. Pure Rust, no new dependencies, reuses `omarchyvr-room` (ORB, stereo, gravity-aware 4-DoF PnP). |
| DBoW2 / DBoW3 / fbow vocabulary | **Not needed at room scale.** Place recognition ranks keyframes by brute-force descriptor hits. With ~100 keyframes per room that takes milliseconds (about 25 ms CPU per frame including ORB on both cameras; 784 frames of a 94 s recording in 2.5 s on 8 threads). DBoW only pays off at thousands of keyframes. Pimax's `voc_GEN_8X4.dbow` is not on Linux, and its format and licence are unknown. The DBoW2/fbow C++ libs (BSD-like) would add a C++ build for no gain here. Revisit for multi-room maps. |
| AprilTag anchor (`omarchyvr-anchor`) | Exists; mm accuracy. But it needs a printed tag in view. Keep it as the precise fallback and the ground truth for the live test. |
| Learned features (XFeat, SuperPoint + LightGlue) / global descriptors (NetVLAD) | Better under lighting changes. Heavier (GPU or a lot of CPU) and a model to ship. A later swap of the per-frame matcher; the rest stays the same. |

## 2. Design (`omarchyvr-roommap`)

1. **build**: a mapping session (an offline recording + its Basalt trajectory, or live from `pimax-cam broker` +
   Monado's head ring).
   * Stereo ORB keyframes (lower pair, cams 0/1, common upright pinhole view, sub-pixel stereo) every 0.2 m /
     15 deg, with points in room coordinates.
   * **Loop closure inside the session.** Each keyframe is relocalised against keyframes more than 12 s away
     (top-K ranking, then RANSAC 4-DoF PnP, then a second pass on the keyframes near the best one). Each success
     is a loop edge. With the VIO odometry between consecutive keyframes, a **4-DoF pose graph** (yaw + xyz per
     keyframe; roll/pitch come from gravity) is solved by robust Gauss-Newton. Outlier loops are dropped (chi > 4).
     Keyframes and points move with their node. The first keyframe stays fixed, so the room frame is the mapping
     session's world.
   * Optional (`--refine`): multi-view re-triangulation of the points along the corrected trajectory.
   * Output: `~/.local/share/omarchyvr/roommap/<name>/`: `map.bin` (omarchyvr-room format), `roommap.toml`,
     `correction.csv` (trajectory correction from the loop closure).
2. **relocalize**: every frame is ORB-matched against the map and gives a per-frame 4-DoF RANSAC hypothesis. Then
   one transform `room_from_slam` (yaw + translation) is **fitted jointly over all correspondences of all frames
   in a 4 s window**. Each frame uses its own tracker pose (Huber LM, robust start = median hypothesis, gate
   1 deg). The fix is accepted when at least 12 frames and 300 inliers agree (at least 60 % of the window).
   Result: `last-reloc.toml`.
3. **apply**: writes `$XDG_RUNTIME_DIR/pimax-anchor.offset` (`pimax-anchor-offset 1`, Monado pid, pose = yaw about
   +Y + translation). The Monado `desk` build with `PIMAX_ANCHOR=1` applies it to the head pose, and WayVR's stage
   lands where it was in the room frame. The same hook as the AprilTag anchor, so only one of the two should
   write it per session.
4. **eval** / **cycle**: the offline validation below.

Frames: the tracker world is y up (Basalt z-up turned by the driver's `SLAM_TO_YUP`), IMU body = OpenXR head axes.
The head ring holds raw SLAM poses before the offset, so a live `relocalize` measures `room_from_slam(raw)`, which
is exactly the offset to write. A live `build` uses the active offset as the room frame. An **offline** map's room
frame is the Basalt replay world of its recording, which is not that session's live app world. So after the first
`apply` the windows are placed once by hand, and from then on they come back.

## 3. Offline results

Recordings (`~/.local/share/omarchyvr/slam-recordings/20261014-*-live`, Basalt replays via `run_slam.sh replay`, all
on 14 Oct):

| rec | length | path | Basalt end-start | content |
|---|---|---|---|---|
| 151524 | 94 s | 25.8 m | 1.14 m (4.4 %) | starts lying on the sofa cushion, walk |
| 152215 | 94 s | 20.9 m | 0.37 m (1.8 %) | same |
| 162452 | 102 s | 40.6 m | 15.3 m (rest dead-reckoning, see slam-drift.md) | walk, then 50 s on the cushion |
| 163703 | 57 s | 7.2 m | 0.60 m (8.2 %) | second area |
| 164420 | 62 s | 7.5 m | 0.10 m (1.3 %) | second area |
| 164606 | 72 s | 23.9 m | 0.32 m (1.3 %) | second area + part of the first |

**Recognition (first fix, 36 map x recording pairs).** Both areas are recognised and there are **no false
positives**. Maps of the walk area (151524/152215/162452) never match the second area (163703/164420), and vice
versa (0-14 single matching frames, fix refused). 164606, which walks through both, matches maps of both. Time to
first fix: **3-7 s** when the session starts with the headset in hand (1637-1646). It is 15-30 s when the
recording starts with the headset lying face-down on the cushion: the cameras see fabric until pickup. Self-checks
(a map relocalising its own recording, first fix): 1-8 mm and 0.03-0.18 deg from identity.

**Accuracy: same-place cycle.** There is no ground truth: the AprilTag came after these recordings. Test used:
sessions A and B, each with its own map. For each 3 s window of B relocalised in map A, take the window of A at
the same place (within 0.5 m, view within 30 deg) relocalised in map B, and compose the two fits. A perfect system
gives identity (A's own loop correction there). The error is measured at the place, so it is what the user would
see. It is about sqrt(2) x a single relocalisation's error. `omarchyvr-roommap eval`:

| pair (windows) | no loop closure: median / p90 | **with loop closure**: median / p90 | yaw median / p90 (loops) |
|---|---|---|---|
| 151524-152215 (25) | 66 / 125 mm | **18 / 50 mm** | 0.37 / 1.0 deg |
| 151524-162452 (5) | 162 / 281 mm | **23 / 59 mm** | 0.25 / 0.35 deg |
| 152215-162452 (4) | 135 / 230 mm | **50 / 79 mm** | 0.78 / 2.1 deg |
| 163703-164420 (7) | 73 / 78 mm | **15 / 21 mm** | 0.08 / 0.28 deg |
| 164606-152215 (4) | 38 / 62 mm | **31 / 32 mm** | 0.36 / 0.61 deg |
| 163703-164606 (3) | 71 / 370 mm | 74 / 371 mm | 0.28 / 0.45 deg |
| 164420-164606 (3) | 166 / 281 mm | 81 / 352 mm | 0.24 / 0.42 deg |

* Loop closure is what made the difference: 3-7x better where the mapping walk revisits places (52-82 loop edges
  in 151524/152215, keyframe corrections up to 20-27 cm / 2.7 deg). Without it the map holds the VIO drift
  between visits, and a later session lands on one copy or the other.
* Multi-view point refinement (`--refine`) was neutral on top of loop closure (18 -> 18 mm, 23 -> 33, 15 -> 14;
  without loop closure 66 -> 54 mm). It stays opt-in.
* 164606 (6 loop edges in a 24 m one-way walk, only 3 matching windows) is the weak case: a map from a walk that
  never comes back cannot be corrected.
* So a single relocalisation is good to about **1.5-3.5 cm and 0.1-0.5 deg (median)**, with a tail to 5-6 cm /
  1 deg on mapped-and-revisited areas. The 4 s first fix pools 12-30 frames, similar to these windows. For
  comparison, the AprilTag anchor is mm-level when the tag is in view.
* Window-to-window noise within a session is a few cm. On top of that comes a slow trend: the yaw of
  `room_from_slam` moves 2 deg and the translation ~10 cm over 75 s. That is the live VIO drifting after the
  fix, not relocalisation error, and it is why section 5 lists a periodic re-fix.

## 4. Expected accuracy and failure modes

* **Accuracy:** 2-3 cm and 0.3 deg typical at the place of the fix, after a loop-closed mapping walk. A window 1.5 m
  away also moves by yaw x distance (0.3 deg -> 0.8 cm, 1 deg -> 2.6 cm). The live VIO then drifts as usual (1-2 %
  of the distance walked), until the next fix.
* **Lighting changes:** ORB (FAST + BRIEF) is fairly invariant to global gain, and AE helps (the synthetic test
  passes at 60 % brightness). Lamps on vs off, or daylight vs night, changes shadows and highlights, so fewer
  matches and a slower fix. A dark room gives no fix at all (the dark test clip has mean 6/255). The tool then
  refuses and leaves the offset alone. Remedy: map once per lighting condition (two maps, try both). The proper
  fix later is to merge sessions into one map.
* **Moved furniture:** points on moved objects fail the joint gate (1 deg) and drop out as outliers. The fix holds
  while the static parts (walls, shelves, desk edge, ceiling lamp) still give 300 inliers. A heavily rearranged
  room means rebuilding the map. Keep the mapping walk looking at walls and fixed things, not only the desk
  clutter.
* **Headset lying on the cushion / cameras covered:** no view, no fix (the 15-30 s on the old recordings). The fix
  comes as soon as the room is in view.
* **Repetitive texture / similar rooms:** the joint fit needs 12 frames and 300 inliers that agree; no false
  positive occurred across the two areas. A risk remains for genuinely repetitive scenes (the same poster twice).
* **Mapping walk with no loop:** no loop closure and an inconsistent map (164606). The walk must come back past
  where it started.
* **Tracker resets mid-session** (the still-hold reset, a loss): Basalt's world jumps. The offset is per Monado
  pid but not per tracker reset, so relocalise again after a reset (see section 5).

## 5. What is left

1. **Live test (needs the user, Monado `desk` with `PIMAX_ANCHOR=1` and 6DoF, broker running):** live `build`
   and `relocalize --source broker`. The code path is written and smoke-tested on a `.pxcam` without a ring, but
   it has never run against a real head ring. Ground truth: the AprilTag (`omarchyvr-anchor detect` before and after a
   roommap `apply`; the tag should land on the same world spot to within a few cm).
2. Continuous mode: re-relocalise every few seconds while tracking, blend the offset change (the
   `omarchyvr-room` session already has the refine/agree logic), and redo the fix after a tracker reset.
3. Launcher/GUI: `omarchyvr desk start` -> `omarchyvr-roommap relocalize --source broker --apply` once tracking runs; a
   "Map my room" button.
4. Merge sessions into the map (lighting variants, moved things), plus a staleness score per keyframe.
5. Cams 2/3 (upper pair) for more coverage once their calibration is fixed (selfcal); DBoW only if maps grow
   past a few hundred keyframes.

## 6. Usage

```sh
# Map the room (live, ~60-90 s): walk slowly through the room, look at every wall, the desk and the floor near
# you, and come back past your starting point at the end (that closes the loop).
omarchyvr-roommap build --source broker --seconds 90 --name home
# or offline: analysis/tools/run_slam.sh record 90 ; run_slam.sh replay DIR ; omarchyvr-roommap build --rec DIR --name home

# Every later session, once 6DoF tracks: look around for a few seconds
omarchyvr-roommap relocalize --source broker --name home --apply      # or relocalize, then: omarchyvr-roommap apply --name home
omarchyvr-roommap info --name home
# the first time after mapping: place the WayVR windows; from then on they come back.

# Floor (analysis/floor.md): build stores it; relocalize --apply / apply put it on Monado's STAGE floor
omarchyvr-roommap floor detect --name home --save | floor touch --name home --save | floor show --name home
# Offline validation
omarchyvr-roommap relocalize --rec REC_DIR --name home                  # first fix, whole session, per-window spread
omarchyvr-roommap eval --map-a A --rec-a DIR_A --map-b B --rec-b DIR_B  # same-place cycle test
```

Tests: `cargo test --release -p omarchyvr-roommap` (6 tests: the joint fit with noise and 20 % outliers recovers
the transform to < 5 mm / 0.05 deg; the pose graph removes injected drift; result/offset file round trip in the
Monado format; synthetic room through the real calibration: a second session relocalises to < 1 cm and another
room is rejected; synthetic drifting mapping session: loop closure pulls the end back to < 4 cm).
