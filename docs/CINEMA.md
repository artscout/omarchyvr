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
| | value |
|---|---|
| width | 11 m |
| distance | 8 m |
| horizontal view | 68 degrees |
| curvature | 0.2 |

A mid-theatre seat: "a little bit far away but big". It needs our WayVR fork with the raised caps (branch
`no-auto-reset`: desk screens up to 20 m wide and 20 m away, `grab_size_max` / `grab_dist_max` 20 m, so a big screen
is not reset as "invalid"). With an older WayVR (6 m / 8 m caps), set `WAYVR_MAX_WIDTH` back to 6.0 in preset.rs and
the preset keeps the same angle at 6 m wide, 4.36 m away.

## Leaving it
"Max, exit cinema" / "exit huge cinema" / "exit imax" / "normal screens": the mode from before comes back with its
passthrough, its screens are placed again, and the background that was on before the huge cinema returns.
"Max, undo" also works. Captions stay off (say "captions on").

## Not done / notes
- Moving the focused video window to the main screen on entry is not included (use "move this to the main screen").
