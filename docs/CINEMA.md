# Huge cinema

"Max, huge cinema" (also: imax, imax mode, big cinema, giant cinema, cinema mode huge, theater mode, make it a huge
screen) turns Work into a real cinema: one very big curved screen a little far away, the room dark.
Switch: app > Desktop > Preset > **Huge cinema** (used at the next Work start), or by voice at any time.
CLI (the engine behind it): `omarchyvr desk start --preset cinema-huge`, `omarchyvr desk preset cinema-huge`
(aliases: `imax`, `huge-cinema`, `theater`, `huge`). The choice is saved with the other Work switches and recorded in
start-argv.txt, so a restart keeps it.

## What it does
- Shows only the main screen; every other VR screen is hidden (never closed: the apps keep running, the toast names them).
- The screen is placed again on every switch (a plain preset switch leaves placed screens alone; this one does not).
- Passthrough off (black room) and the WayVR backdrop/skybox off. The background you chose in the app is not applied
  at a start in this mode (it stays saved). Captions are turned off (voice entry).
- One source of truth for the numbers: `huge_size()` in `rust/omarchyvr-tools/src/vr/preset.rs`.

## Numbers
| | wish (mid-theatre seat) | sent to WayVR today |
|---|---|---|
| width | 11 m | 6 m |
| distance | 8 m | 4.36 m |
| horizontal view | 68 degrees | 68 degrees (same angle) |
| curvature | 0.2 | 0.2 |

The wish is "a little bit far away but big". WayVR (our fork) clamps `desk-layout` / `desk-screens` width to 0.3..6 m
and distance to 0.4..8 m (`ipc/events.rs`), and `grab_size_max` 6 m (x1.1) makes `window.rs` auto-reset a bigger
screen. So today the screen keeps the angle but comes closer: 6 m at 4.36 m. Same view size, less "far away" depth.
Follow-up: raise those caps in WayVR (width to 12 m, distance to 10 m, `grab_size_max`/`grab_dist_max`), then set
`WAYVR_MAX_WIDTH` to 12.0 in preset.rs and the preset sends 11 m at 8 m by itself.

## Leaving it
"Max, exit cinema" / "exit huge cinema" / "exit imax" / "normal screens": the mode from before comes back with its
passthrough, its screens are placed again, and the background that was on before the huge cinema returns.
"Max, undo" also works. Captions stay off (say "captions on").

## Not done / notes
- Moving the focused video window to the main screen on entry is not included (use "move this to the main screen").
