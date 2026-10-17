# Play: the graphics card for the game, light game mode, games that crash with xrizer

## Play frees the graphics card (user decision 2026-10-17)
Play keeps everything a game needs (Monado, head 6DoF with the camera broker and SLAM, controllers and their optical
tracking, sound) and nothing of Work: no WayVR desk, no VR screens, no voice, no subtitles, no local AI, no speech
server. Before `omarchyvr vr start` starts anything it stops, by pid only, what a crashed or forgotten Work left and
every `omarchyvr-ai` slot (`work::clear_for_play` in rust/omarchyvr-tools/src/vr/work.rs); a running session is never
touched, Sunshine never. What it stopped is written to `/tmp/omarchyvr-session/gamemode-paused.txt` and shown on Home
("Stopped for Play: local AI, voice"). Nothing is started again when Play stops: the AI and voice belong to Work and
start with the next Work. `omarchyvr-ai up`/`serve` refuse while Play runs (`--force` for agents), omarchyvrd never
wants a Work component (`ai.server`, voice, voicehud) in a Play session and stops one it finds. Home shows, while Play
runs, what uses the graphics card (`nvidia-smi --query-compute-apps`, read only, every 10 s): "nothing of Work or the
AI uses it: 17.0 of 24.0 GB free", or the leftover by name and pid. Audit and evidence: docs/PLAY-WORK-AUDIT.md.
The old "Game mode" switch (pause the local AI during Play, `pause_ai`) is gone from the app: Play always does it.

Work -> Play from Home ("Switch to Play") is one job: Work's full stop (the same as Exit Work, docs/WAYVR-CONTROLS.md),
the wait until Monado is gone, then Play's start.

## Eye tracking in games
Switch: Home > quick settings and Display > Performance, "Eye tracking in games" (default off). Saved as
`eye_in_games` in `~/.config/omarchyvr/gamemode.toml` (`omarchyvr vr gamemode eye on|off`). Play passes `--eye` or
`--no-eye` to `omarchyvr vr start` and `start-argv.txt` keeps it; `vr start` without either flag uses the saved switch.
On: the eye tracker runs in the game session too (visual only, like in Work). Off: Play leaves the eye tracker off.
It applies to the next Play. Work's own switch (Quick settings "Eye tracking") is separate.

## Light game mode (Play)
Switch: Home > quick settings and Display > Performance, "Light game mode" (default on). Saved as `light_game` in
`~/.config/omarchyvr/gamemode.toml`, shared with the CLI (`omarchyvr vr gamemode light on|off`).
Play passes `--light` (or `--no-light`) to `omarchyvr vr start`, so `start-argv.txt` keeps it for every restart;
`vr start` without either flag uses the saved switch.

What Play runs with it on: monado-service (compositor), the Pimax driver, camera broker + SLAM (head 6DoF),
optical controller tracking, controllers and haptics, headset sound/mic routing, the saved backlight (and the eye
tracker when "Eye tracking in games" is on).
What `--light` drops: the wrist readout (`--perf`: a second WayVR plus omarchyvr-perf), the room anchor hook
(`PIMAX_ANCHOR`) and the room search after start. Eye tracking is no longer part of it (its own switch above).
Never in Play: voice (all voice processes), passthrough, the desk, the local AI. Camera hands only when the user
switched them on. The recorder and its Monado tap run only when the user starts a recording.
`XRT_COMPOSITOR_OUTPUT_GAMMA` is only set when the user picked Bright/Brighter (Display); with Standard there is no extra pass.
omarchyvrd follows the argv (`--light` means no perf and anchor; `--eye`/`--no-eye` the eye tracker).

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
* **Why Alyx still ran with xrizer (2026-10-17 14:50)**: the list is only read by `vr start --game APPID`, and Play passed
  `--game` only when "Next Play is for this game" was ticked, which the app sets after a crash. The user switched "Games
  run with" to Monado (job at 14:48:44, openvrpaths -> xrizer) and pressed Play (`vr start ...` without `--game`), then
  started Alyx from Steam: Steam, not the app, launches games, and a running Monado session cannot be re-routed per game
  (SteamVR and Monado cannot share the headset; pointing openvrpaths at SteamVR would make every other game start
  vrserver against the running Monado). The seed list alone therefore never helped.
* Fix: (1) Play now passes `--game` by itself when a listed game is already running (`AppCore::start_game`: the picked
  game, else the running listed game). (2) When a listed game runs under Monado + xrizer, Home shows a warning with the
  button "Switch Play to SteamVR for this game": one job stops the session (by pid), waits for Monado to go and runs
  `vr start --game APPID` (SteamVR for this start only). Then close the game if it is still open and start it again
  from Steam. Limit: the game that was already launched cannot be saved (xrizer is loaded by the game itself); the
  offer saves the next launch. Safest habit: press Play for a listed game first, or tick "Next Play is for this game".
