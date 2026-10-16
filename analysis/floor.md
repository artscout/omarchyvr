# Floor: detection, Monado floor, drift lock, floor grid (2026-10-15)

The complaint: "we always end up under the floor". Code: `rust/omarchyvr-roommap/src/floor.rs` (detection, storage,
follower), `floorcmd.rs` (`omarchyvr-roommap floor ...`), `rust/omarchyvr-passthrough/src/floorgrid.rs` (grid in VR), the
Room page (`rust/omarchyvr-app/src/pages/room.rs`, `rust/omarchyvr-gui-core/src/room.rs`), and the Monado commit
`6094c0c` on `desk` (local only, never pushed). Everything below ran offline on recordings and the saved `home` map.
Nothing touched the live session.

## 1. Why the floor is wrong today

Monado (pimax driver, legacy builder) never learns the floor:

* `U_DEVICE_ALLOCATE` leaves the tracking origin as `XRT_TRACKING_TYPE_NONE`, so `u_builder_setup_tracking_origins`
  gives it a "nominal height" `initial_offset.y = 1.6` (`u_builder_helpers.c:60-65`). STAGE is a null space at the
  root, and LOCAL_FLOOR = (local.x, 0, local.z) (`b_space_overseer.c:1403-1460`).
* The 6DoF tracker (Basalt) starts its world at the headset's pose when tracking starts. So the reported head height
  is `1.6 + slam_y (+ offset_y)`: the floor ends up 1.6 m below wherever the headset was at start.
  * Start standing with the eyes at 1.70 m: the virtual floor is 10 cm above the real one, and you stand in it.
  * Start seated (eyes at about 1.2 m) or with the headset on the sofa cushion (0.45 m, as in the recordings): the
    virtual floor is 0.4-1.15 m too low.
* The room map makes it no better. Its room frame is the mapping session's tracker world, so `relocalize --apply`
  carries the mapping session's start height into every later session. In `home`, the floor sits at -0.42 m in the
  room frame, so without a correction it lands 1.18 m below STAGE.
* On top of that comes vertical drift (section 4): 1-8 cm per minute and a few cm per session, in either direction.

The fix uses the existing hook. The `PIMAX_ANCHOR` offset already carries a full translation, Y included, and a full
quaternion (`pimax_anchor.h`: `app = offset * slam`, applied to the head, and through the head to controllers and
gaze). A floor at room height `f` lands on STAGE y = 0 when the offset's y gets `-1.6 - f` added
(`floor::floor_shift`). `origin_height()` also adds `XRT_TRACKING_ORIGIN_OFFSET_Y` when the Monado process has it
set (it reads `/proc/<pid>/environ`). No new mechanism and no Monado change is needed for the floor itself.

## 2. Floor detection

Frames are gravity aligned (y up), so the floor is `y = h`.

* **Points (`floor::detect`):**
  1. A 1 cm height histogram, smoothed, gives the peaks: horizontal layers.
  2. Each layer is refined by a gravity-constrained RANSAC plane (tilt at most 4 deg, inliers within 2 cm, samples
     spanning at least 30 cm).
  3. **The floor is the lowest layer** with these properties:
     * at least 60 inliers, and at least 8 % of the strongest layer;
     * a p10-p90 area of at least 0.4 m²;
     * 0.4-2.1 m below the median head.
  4. Desks and tables are higher layers. Sparse layers below the floor (reflections, mismatches) fail the support
     test.
  5. Height = **level median** of the inliers. The fitted tilt (0.1-1.7 deg on these recordings, tracker attitude
     error or map bend) is reported but not extrapolated: the offset can only shift the world, not tilt it. In
     `home`, extrapolating the tilt moved the height by 1.7 cm between point ranges; the level median moved by
     0.7 cm.
  6. Confidence 0..1 is built from support, area, flatness, noise, a usual head height, and a competing lower layer.
* **Source of points:**
  * `build` uses the dense stereo points of every 3rd mapping frame (a few thousand floor points), moved by the
    loop-closure correction. It saves `floor.toml` and `preview.txt`.
  * `floor detect --save` uses the map's keyframe points. That is the fallback for maps built before this change,
    such as `home`. Those points are sparse on the floor (79-430 inliers): fine for `home`, wrong for the 17-keyframe
    map 163703 (it took the sofa, confidence 0.17). A touch or manual floor is kept unless the points agree with it
    within 3 cm.
* **Controller touch (`floor touch`):**
  1. The optical controller pose (ctrl-track shm) goes through the head ring into the tracker world.
  2. 1 s of stillness (90 % within 6 mm), at least 0.5 m below the head; no trigger is needed.
  3. Floor = the median origin height minus a bottom offset (`--bottom`, default 35 mm, the ring/grip radius,
     roughly +-1 cm).
  4. The result is converted to the room frame through this session's fix and combined with the points floor:
     precision-weighted when they agree within 4 cm, the touch wins otherwise.
* **Manual (`floor set --y`)**, or standing eye height (`floor set --standing 1.65`: head now minus your eye height).
* **Live tracking (`floor::track`):** for short windows of live points, a robust mean shift (5 cm, then 2.5 cm
  radius) starts from the known floor, moves at most 15 cm, and needs a layer denser than its 5-10 cm shells and
  0.25 m² of area. With the lowest-layer rule, windows jumped to knees and laptops (10 cm errors). Tracking cut the
  window-to-window noise to 1-2 cm.

## 3. Offline accuracy

**Single map, repeatability.**
* `home` (160 keyframes, 15 Oct): the floor is at -0.4207 m. Point ranges of 2.0 / 3.0 / 3.5 / 4.5 m give -0.420 /
  -0.414 / -0.421 / -0.421 m, a 7 mm spread. Formal sigma 0.8 mm, inlier spread 14 mm (stereo noise). The head is
  1.41 m above it on the mapping walk.
* Whole-recording floors are below. Each height is relative to that recording's own start, which is why they differ.

| rec | floor (tracker world) | points | area | head above (median) | what the cameras show |
|---|---|---|---|---|---|
| 151524 | -0.396 m | 6436 | 3.3 m² | 1.45 m | walk, standing |
| 152215 | -0.320 m | 6187 | 1.5 m² | 1.47 m | walk, standing |
| 162452 | -0.633 m | 2970 | 16 m² | 1.05 m (50 s on the cushion) | walk, cushion |
| 163703 | -0.243 m | 2980 | 3.2 m² | 0.88 m | seated on the sofa |
| 164420 | -0.437 m | 2886 | 1.2 m² | 0.97 m | seated, looking at knees, rug and laptop (checked on the images) |
| 164606 | -0.413 m | 4198 | 6.2 m² | 1.11 m | mixed |

**Plausibility checks:**
* The headset resting on the sofa cushion reads 0.45-0.48 m above the floor in 164420, 164606 and 163703, which is
  the cushion height.
* Standing walks read 1.45-1.47 m for the head in both 151524 and 152215, a 2.6 cm difference between sessions.
* Seated readings are 0.88-0.97 m.

**Across sessions.** Compare the floor of map A with the floor of recording B moved into A by B's first room fix
(t_y), plus B's loop correction at the fix (16 fixes, `xfloor.py` in the session scratch):

| target map | pairs | \|difference\| |
|---|---|---|
| `home` (160 kf) | 151524, 152215 | **4.8, 3.0 mm** |
| 164606 (129 kf) | 151524, 152215, 164420 | 12.1, 15.1, **4.3 mm** |
| 151524 / 152215 / 162452 (63-95 kf) | 6 | 18-109 mm |
| 163703 / 164420 (17-31 kf) | 5 | 54-175 mm |

* Against a good map the floor comes back within 3-15 mm.
* The large differences go both ways for the same pair (151524 in 152215: +109 mm; 152215 in 151524: -101 mm). That
  is the vertical error of the first fix in short maps ("first fix vs whole session" was 17-29 cm there), not of the
  floor. This is why the floor is also locked live (section 4): it does not depend on how good the relocalisation
  is vertically.

## 4. Vertical drift and the floor lock

The floor in the tracker world should be constant, so its change over time **is** the tracker's vertical drift.
`floor drift --rec`: 6 s windows every 2 s, tracked from the whole-session floor; only windows that see the floor
count. The "lock" column is the `Follower` (time constant 10 s, at most 10 mm/s, outliers over 5 cm held until 3
agree) run **causally**: each window is scored with the correction from earlier windows only.

| rec | windows | drift vs first window: median / p90 / max | trend | with the lock: median / p90 / max |
|---|---|---|---|---|
| 151524 | 22 | 40 / 61 / 83 mm | +1.0 cm/min | **16 / 37 / 58 mm** |
| 152215 | 19 | 25 / 47 / 48 mm | +1.0 cm/min | **14 / 34 / 50 mm** |
| 162452 | 14 | 20 / 36 / 46 mm | -1.7 cm/min | 23 / 39 / 41 mm |
| 163703 | 15 | 12 / 21 / 21 mm | -3.7 cm/min | **6 / 12 / 13 mm** |
| 164420 | 14 | 24 / 43 / 45 mm | +0.9 cm/min | **13 / 26 / 68 mm** |
| 164606 | 22 | 17 / 50 / 68 mm | +2.9 cm/min | 18 / 36 / 47 mm |

* The drift is a few cm per session, in either direction (no systematic sinking), with 1-4 cm/min trends over these
  60-100 s recordings. Over a 30-60 min session that is enough to end up under, or above, the floor even when the
  start was right.
* The lock halves the median error where the drift is a trend, and helps less where the window measurement noise
  (1-2 cm: a short window sees 150-700 floor points) is as large as the drift. The residual is at the measurement
  noise level, not growing with time.
* Smoothness: the follower moves the applied offset at most 10 mm/s, i.e. 0.11 mm per 90 Hz frame (unit test:
  never more than rate/90 per frame). It writes the offset file at 4 Hz, because Monado re-reads it at most every
  250 ms. So without the Monado patch a step is at most 2.5 mm; with the patch the head glides.
* CPU: ORB + stereo on 1 frame in 3 (about 23 ms each) is about 15 % of one core.

## 5. Monado side

* **Floor:** no change. `omarchyvr-roommap relocalize --apply` and `apply` now add the floor shift from the map's
  `floor.toml` (`--no-floor`: old behaviour). The output says how much.
* **Optional glide patch:** `forks/monado` commit `6094c0c` on `desk` (local only). It changes the
  `pimax_anchor.h` hook:
  * The applied offset moves toward the file's at most `PIMAX_ANCHOR_SLEW` m/s (0.05) and `PIMAX_ANCHOR_SLEW_DEG`
    deg/s (2).
  * The first offset, and changes over `PIMAX_ANCHOR_SNAP` (0.25 m) or 5 deg, are applied at once (a deliberate
    placement).
  * `PIMAX_ANCHOR_SLEW=0` gives the old behaviour.
  * Only `pimax_hmd.c.o` was compile-checked, in `~/.local/lib/omarchyvr/monado-floorcheck`. The live build was not
    touched.

## 6. Floor grid in VR

`omarchyvr-passthrough run --floor` (or the `o` key, or `$XDG_RUNTIME_DIR/omarchyvr-floor-grid` = `1`/`0`, read once a
second; the Room page has the switch):
* It draws one OpenXR quad layer in STAGE space at y = 0: 6 x 6 m, a 1024² premultiplied texture, 0.5 m cyan lines.
* The lines fade out from 0.8 m to 2.8 m and are cleared under your feet.
* The texture is made and uploaded once. Per frame only the quad pose changes (it follows the head in whole cells,
  so the lines stay fixed in the room).
* GPU cost: one textured quad in the compositor.
* With passthrough on, the grid is drawn over the camera image, which makes it a direct check: the grid lines should
  sit on the real floor.

## 7. App: Room page

The steps:
1. Map the room: the keyframe count and a quality chip.
2. Recognise the room: "recognised in this session".
3. Floor: the stored height and its source with a % confidence, plus the actions "Find floor in the map", "Touch
   floor with controller" (left/right), "Set from my height" (standing eye height) and a manual height. Also the
   floor grid switch.

Below the steps:
* **Map preview:** top-down and side views. Floor points are tinted, points below the floor are red, and the mapping
  walk is drawn. "You" is shown live from the head ring through this session's fix, refreshed every 3 s with the room
  state, with the head height above the floor.
* **Saved rooms:** the maps, with keyframes and floor; a click selects one.
* **AprilTag:** unchanged.

Floor actions are jobs of kind `room:floor`, so they never touch the offset owner (one writer per session stays).
Demo mode shows a synthetic room. `PIMAX_ROOM_SHOT=DIR cargo test -p omarchyvr-app room_page_picture -- --ignored`
renders the page.

## 8. Live test (needs the user)

1. Install `omarchyvr-roommap`, `omarchyvr-passthrough` and `omarchyvr-app` from `rust/target/release` the usual way. They are
   only built in the repo target so far.
2. `omarchyvr desk start --6dof --ctrl-track --anchor` (the tag test's flags), then wait until tracking runs.
3. `omarchyvr-roommap floor detect --name home --save`: offline, writes `home/floor.toml`. Expect about -0.42 m.
4. `omarchyvr-roommap relocalize --source broker --name home --apply`: look around. The output should print
   "floor: ... puts the room -1.18 m vertically".
5. `omarchyvr-roommap floor show --name home`, standing upright: "head now: X m above Monado's STAGE floor" should be
   your eye height (about 1.5-1.7 m). Sit down: about 1.1-1.25 m.
6. Floor grid: `omarchyvr-passthrough run --floor` (or `echo 1 > $XDG_RUNTIME_DIR/omarchyvr-floor-grid` with passthrough
   running). The lines should lie on the real floor; put a controller on it and look.
7. Touch refine: `omarchyvr-roommap floor touch --name home --hand right --save`. Lay the controller flat on the floor
   in view and let go. It prints the touch vs points difference: expect under 2 cm.
8. Drift lock: `omarchyvr-roommap floor follow --dry-run --seconds 60` prints the measured floor and target every 2 s.
   Without `--dry-run` it writes the offset (smoothly). Leave it running for a long session and check with `floor
   show` that the head height stays put.
9. With the Monado patch relinked: `PIMAX_ANCHOR_SLEW` defaults apply. The Monado log shows "offset placed at once"
   only for the first fix and big re-fixes.

## 9. Open

* `floor follow` is a manual command. Next: start it from the launcher or the app after Recognise (it needs one
  owner rule: it only edits the y of whoever wrote the offset).
* `relocalize` could measure the floor live in its own frames and prefer it when the map's vertical fix is weak
  (short maps, section 3).
* The touch bottom offset is a constant 35 mm. With the controller's LED model and its orientation, the lowest shell
  point could be computed instead.
* A floor plane per keyframe region for large or uneven floors (steps, rugs: the rug in 164420 is about 1 cm).
