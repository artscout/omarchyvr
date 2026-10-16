# Light game mode and games that crash with xrizer

## Light game mode (Play)
Switch: Home > quick settings and Display > Performance, "Light game mode" (default on). Saved as `light_game` in
`~/.config/omarchyvr/gamemode.toml`, shared with the CLI (`omarchyvr vr gamemode light on|off`).
Play passes `--light` (or `--no-light`) to `omarchyvr vr start`, so `start-argv.txt` keeps it for every restart;
`vr start` without either flag uses the saved switch.

What Play runs with it on: monado-service (compositor), the Pimax driver, camera broker + SLAM (head 6DoF),
optical controller tracking, controllers and haptics, headset sound/mic routing, the saved backlight.
What `--light` drops: eye tracking (`PIMAX_EYE`), the wrist readout (`--perf`: a second WayVR plus omarchyvr-perf),
the room anchor hook (`PIMAX_ANCHOR`) and the room search after start. Already never in Play: voice (all voice
processes), passthrough, the desk. Camera hands only when the user switched them on. The local AI is paused by
"Game mode" (separate switch). The recorder and its Monado tap run only when the user starts a recording.
`XRT_COMPOSITOR_OUTPUT_GAMMA` is only set when the user picked Bright/Brighter (Display); with Standard there is no extra pass.
omarchyvrd follows the argv (`--light` means no perf, anchor, eye).

## Games that crash with xrizer (xrizer is not changed)
xrizer aborts a game that calls an OpenVR function it lacks (Half-Life: Alyx: `GetPosesForFrame` is `todo!()`).
* Evidence: xrizer's log (truncated at each game start) is copied to `~/.local/state/omarchyvr/xrizer-logs/<unix>-<appid>.txt`
  (newest 20) when a game ends and before each `vr start`.
* Detection: the app sees the Steam game (`reaper SteamLaunch AppId=N`). When it ends, a panic in the xrizer log, or a
  SIGABRT core dump (`coredumpctl info`) with xrizer / vrclient.so frames, saves the game as "Runs with SteamVR"
  and shows a message on Home. An abort without xrizer evidence is not blamed on xrizer.
* List: `~/.config/omarchyvr/game-runtime.json` (built-in seed: Half-Life: Alyx 546560; user and crash entries win).
  `omarchyvr vr game-runtime list|set APPID steamvr|xrizer|default [NAME]|next APPID|none|archive|check APPID SINCE`.
* Runtime choice: Monado + xrizer and SteamVR cannot share the headset, and Steam (not the app) launches games, so the
  choice is per session: Ready check > "Games that run with SteamVR", tick "Next Play is for this game" (set
  automatically after a crash); Play then runs `vr start --game APPID`, which starts that session with SteamVR for this
  start only (global "Games run with" unchanged). The next normal Play puts xrizer back (`ensure_xrizer`).
