# Room setup: floor, play area, centre and forward

Goal (user, 2026-10-15): "all this ready in our GUI: quick setup and fast setup, then make SteamVR know that too."
The room setup is the same on every start, for Monado games, WayVR (Work) and SteamVR.

## What the user does (app only)

Home > **Room** > Room setup. Two buttons. Both are guided **in the headset**: a text panel in front of you, the
`pimax-hud` overlay over WayVR. Every step also goes to the app's Output panel, and the result shows in the Room row.
If Work is not running with 6DoF and the room anchor, the button starts it first (the hand test does the same).

**Fast setup (about 30 s)**

1. Stand in the middle of your play space. Look around slowly: left, right, behind you, at the floor.
   - If the room is already saved, it is recognised.
   - If not (or it does not match), a 30 s map is made.
2. The floor comes from the map points. If the map has no floor, the panel asks you to lay the right controller on
   the floor.
3. Look straight ahead, where games should face. Pull a trigger, or pinch, or hold still for 3 seconds.
4. The play area is the free space the map shows around you. Each side grows from 0.4 m up to 2.5 m and stops at
   furniture or walls (map points 0.15 to 2.0 m above the floor), or where the map ends. With no map points it is
   2 x 2 m.

**Full setup (about 3 min)**

1. Walk the room for 60 s: every wall, the furniture, the floor. A new map is made.
2. Lay the right controller flat on the floor and let go. It confirms the floor the map found, or corrects it.
3. Hold a trigger (or pinch) and walk along the edge of your free space. Let go when you are back at the start. The
   loop also closes by itself when you come back within 30 cm after more than 3 m. A trace under 2 m starts again. The
   trace is cleaned up: points 5 cm apart, simplified to 5 cm, and replaced by its convex hull if it crosses itself.
   It must be at least 1 m².
4. Stand in the middle and look forward: trigger, or hold still.

The room name is the app's Room name field (the map folder). Run either setup again any time, e.g. after moving
furniture.

**Switches** (saved in `~/.config/pimax/room.toml`, shared with `pimax vr room ...`):

- **Put my room in place at every Play and Work** (default on, once a room is saved).
- **Tell SteamVR my play area and floor** (default on).

**Status line** in the Room row. Examples:

- `Room "home": play area 2.6 x 2.0 m, floor set, Room "home" in place after 6 s (floor, centre and forward ...)`
- `... not recognised: the world starts where the headset is. Look around the room and press Recognise room, or run
  Fast setup again`

## Files

| File | What |
|---|---|
| `~/.local/share/pimax/roommap/<room>/room-setup.toml` | label, mode, centre (x z), forward `yaw_deg`, the boundary polygon (room frame of the map), `play_area` (width x, depth z), `head_m` |
| `<room>/floor.toml` | the floor height (existing; `analysis/floor.md`) |
| `<room>/last-reloc.toml` | the room fix of the current session (existing) |
| `$XDG_RUNTIME_DIR/pimax-room.status` | `state` (searching, ok, failed, skipped, off), `room`, `pid`, `text`, `when` |
| `$XDG_RUNTIME_DIR/pimax-stage-bounds` | `pimax-stage-bounds 1` / `pid N` / `size W D`: Monado's STAGE bounds |
| `~/.config/pimax/room.toml` | `auto`, `room`, `steamvr` |
| `~/.local/share/Steam/config/chaperone_info.vrchap` | SteamVR's room setup; ours is universe `2` |
| `.../chaperone_info.vrchap.pimax-backup` | SteamVR's own file, from before our first write |

## How it works

**Setup frame.** The origin is the play area's centre on the floor. -Z points forward and y is up. The room offset
Monado applies (the `PIMAX_ANCHOR=1` hook) is now:

```
offset = setup_from_room * floor_shift * room_from_slam      (pimax-roommap setup::final_offset / write_offset)
```

So once the room is recognised, the app world *is* the setup frame:

- OpenXR STAGE origin = the play area centre, floor at y = 0, forward = -Z;
- SteamVR's raw tracking space is the same (driver_monado reports the same Pimax driver poses).

A room without `room-setup.toml` keeps the old offset: floor only, map origin.

**At every start** (`pimax vr start` on both paths, `pimax desk start`, also through pimaxd):

- With `auto` on and the room's `map.bin` there, the start gets `PIMAX_ANCHOR=1`. `room::auto_anchor` handles it in
  `parse_options`, so it works whatever the app's anchor toggle says.
- After the start, `pimax-roommap auto --name <room>` runs detached at nice 15. It writes `<session>/room-auto.pid` and
  `room-auto.log`. It:
  1. waits for Monado's head ring (the hook recreates it at every start);
  2. leaves the session alone if the AprilTag owns this session's offset;
  3. relocalises in 15 s windows for up to 120 s;
  4. applies floor + centre + forward, writes the STAGE bounds and the owner file (`roommap <pid>`);
  5. writes the status file.
- Stop ends it by pid (SIGTERM, only if its name is `pimax-roommap`) if it is still running.
- Failure is a status line. VR keeps going.

**The head-pose feed** (`$XDG_RUNTIME_DIR/pimax-anchor.head`, live failure 2026-10-16):

- The ring is only trusted when its writer pid is the running `monado-service` (or SteamVR's `vrserver` when no
  monado-service runs) and its newest sample is under 5 s old (`pimax_anchor::world::feed_now`). Otherwise the setup
  stops at once and says why in plain words: missing, left over from a closed Monado, taken over by another program
  (pid named), or stopped N s ago. A lost feed after a failed recognition stops the setup instead of mapping anew
  without poses. `pimax-anchor status` prints the same sentence.
- The setup's in-headset panel opens the running Monado's runtime (its build's `openxr_monado-dev.json`), never the
  system's `active_runtime.json`. `pimax-hud` refuses any other runtime while a monado-service runs: on 2026-10-16
  the panel followed `active_runtime.json` to SteamVR, SteamVR's Monado driver opened the headset a second time
  inside vrserver, deleted the offset file, recreated the ring with vrserver's pid and closed; monado-service kept
  writing into the deleted file, so the setup saw "no head movement" and "0 with a Monado head pose (pid 182552)".
- Monado (marker `PIMAX_ANCHOR_OWNER`): a process that finds a fresh ring of another live writer leaves the ring and
  the offset alone (room anchor off there, logged); the ring is created under a temporary name and renamed into
  place; once a second the writer checks the file is still its own and takes it back when it was replaced or
  removed and no live writer keeps it fresh. Basalt restarts ("SqrtKeypointVioEstimator restarted", logged as
  "tracker re-zeroed while still") do not touch the ring.

**SteamVR** (`pimax vr start` with Games run with: SteamVR, before SteamVR is launched, `room::write_chaperone`):

- driver_monado reports `Prop_CurrentUniverseId_Uint64 = 2` for the HMD and both controllers. It is a constant in
  `steamvr_drv/ovrd_driver.cpp`, so no Monado change is needed. This machine's own `chaperone_info.vrchap` (written by
  SteamVR's room setup at 17:28) has `universeID "2"`, version 5.
- The universe-2 entry is replaced. Every other universe is kept. The first write backs the file up. If the content
  is unchanged apart from the time, nothing is written. A file that is not JSON is left alone, with a line saying so.
- The entry:
  - `standing` = identity: raw space is already the setup frame, so there is no sign convention to get wrong;
  - `seated` = (0, head height at setup, 0);
  - `play_area` = [width, depth];
  - `collision_bounds` = one quad per boundary edge, 2.43 m high (bottom/top of one corner, top/bottom of the next),
    the same layout as SteamVR's own file.

**Monado path, OpenXR games.** Monado had no STAGE bounds: `xrGetReferenceSpaceBoundsRect` went to the compositor,
which does not implement it, so games got `XR_SPACE_BOUNDS_UNAVAILABLE`. Monado commit `31687b8` (branch `room`,
marker `PIMAX_STAGE_BOUNDS`) answers STAGE from `$XDG_RUNTIME_DIR/pimax-stage-bounds` when its pid is this
monado-service. The file is read on every call, so the bounds appear the moment the room is in place. The play
area rectangle is centred on the STAGE origin, which is what OpenXR asks for.

**In-headset guide.** `pimax-hud` has a new `Opts::input` option. It adds one action set:

- a float "select": Touch `trigger/value`, simple `select/click`, Index `trigger/value`, and
  `hand_interaction_ext` `pinch_ext/value` when offered;
- the aim pose of both hands;
- head and aim poses located in STAGE.

Monado keeps overlay sessions focused, so the overlay gets input. STAGE samples are turned into the room frame:
`room = room_from_slam * offset^-1 * (stage - (0, origin height, 0))`. Without the panel, the head comes from Monado's
head ring, and forward by holding still still works.

## Low light (night) and lighting changes

Live failure 2026-10-15 22:40 (Work, night, one monitor lit): Fast setup did not recognise the morning map (0 of
626 frame sets matched) and the 30 s map made **0 keyframes from 751 posed frames**. With an empty map every frame
wants a keyframe (no head motion needed), so no frame had the 40 stereo points a keyframe needs. Measured from the
broker (`pimax-roommap light`): brightness **8/255**, auto-exposure pinned at its ceiling (exposure 4108, gain 215,
the largest values Windows uses; ~1/5 of the power-on 10410/449 product), **0 stereo points** in 30 frame sets.
Daylight (`hands-lit.pxcam`): 31/255, 87 stereo points, 30 of 31 frame sets usable.

What changed:
- **Low-light stretch before ORB** (`pimax_room::stereo::LOW_LIGHT`, used by mapping, recognition and the session):
  a frame whose 99th percentile is below 100 is box-filtered 3x3 and stretched (p99 to 200, at most 6x). Lit frames
  are untouched. ORB descriptors are pixel-pair comparisons, so stretched night frames still match daylight keyframes.
  Daylight recording darkened to 8/255 with sensor noise: 8 of 31 frame sets usable without, 26-28 with; synthetic
  room at 12 % light: 631 -> 826 stereo points, and a night session relocalises on a daylight map (test).
- **Setup waits for the headset to be worn** (head pose moves 8 deg or 15 cm within 3 s, up to 60 s): the panel says
  "Put the headset on and look around".
- **Light check** (2 s of broker frames, repeated up to 30 s while too dark): "Too dark for the cameras: turn on the
  room lights" with the points in view and the brightness.
- **Live mapping feedback** on the panel about once a second: views mapped, points in view, and "turn on the lights" /
  "look at furniture" / "turn your head further". A dark or plain room maps up to 30 s longer until 8 views are in.
  A map with no keyframe says why (too dark with the numbers, or too little detail).
- **The map learns other lighting**: after Fast setup recognises the room, 12 s of the current views are added to the
  map in the recognised room frame (`build --append`; `kf_per_session`: only this session's keyframes block a new one
  nearby). A failed recognition that makes a new map keeps the old one as `map.bin.prev`.
- No text-to-speech engine is installed: "spoken" feedback is the in-headset panel text.

Not changed: the auto-exposure ceiling (`PIMAX_CAM_AE_EXP_MAX`/`_GAIN_MAX`, Windows' 4108/215) because Basalt uses the
same frames live; raising the gain ceiling towards 449 would roughly double night brightness without motion blur and
needs a live A/B in a dark room first.

## Engine (agents, tests)

- `pimax-roommap light [--source broker|FILE.pxcam] [--seconds 5] [--no-stretch] [--darken F --noise G]`: can the
  room be mapped now (brightness, exposure/gain, ORB corners, stereo points); `--darken` simulates a darker room.

```
pimax-roommap setup fast|full [--name NAME] [--label TEXT] [--no-hud] [--map-seconds S]
pimax-roommap setup show [--name NAME]
pimax-roommap auto [--name NAME] [--wait S] [--status FILE]
pimax vr room [status|on|off|steamvr on|off|name NAME]
```

Tests:

- pimax-roommap `setup` (geometry, frames, final offset, TOML, the guided steps against a scripted fake input) and
  `setupcmd` (STAGE -> room conversion, wrap, bounds file);
- pimax-tools `vr::room`: settings, setup frame equal to pimax-roommap's, vrchap keeps other universes and backs up
  once, status, time format;
- pimax-gui-core `roomsetup`: setup command and `room-setup.toml` parsing.

## Not done / risks

- The floor in SteamVR assumes driver_monado's raw space has the same 1.6 m tracking origin as monado-service's STAGE
  (same `u_builder_setup_tracking_origins`, offset 0). Check in the headset that SteamVR's floor matches the real one.
- Under SteamVR the room is found by `pimax-roommap auto` as in Monado (broker + head ring of vrserver). The setup
  itself runs in Work (the panel is an OpenXR overlay on Monado).
- The boundary needs tracked controller positions: optical controller tracking (`--ctrl-track`, which Full setup turns
  on) or camera hands (pinch).
- The overlay's trigger also reaches WayVR (Monado sends input to every focused session). Point away from screens
  while tracing.
- Not yet tried in the headset.
