# Render quality and frame health

## The switch
**Display** > **Render quality**: Quality / Balanced / Performance. Saved as `scale_pct` in
`~/.config/pimax/gui-state.toml`; the next Play or Work passes it as `XRT_COMPOSITOR_SCALE_PERCENTAGE` (the CLI reads
the same variable, or `SCALE`). Developer mode adds a custom slider (50-200 %).

| Preset | Scale | Per eye (game render size) | Pixels |
|---|---|---|---|
| Quality | 100 % | 3840x3552 (the panel's own resolution) | 100 % |
| Balanced | 85 % | 3264x3019 | 72 % |
| Performance | 70 % | 2688x2486 | 49 % |

## How it reaches the game
- **Monado (xrizer)**: Monado's recommended view size = panel size per eye (`display 3840x3552` after the 90/270
  degree pre-rotation) x scale (`comp_compositor.c`, clamped to 200 %). xrizer passes it to the game as the
  OpenVR recommended render target size. The game may scale again with its own setting.
- **SteamVR**: the Pimax driver's `GetRecommendedRenderTargetSize` = the same size x scale. SteamVR's Auto
  resolution multiplies it again (on an RTX 4090 up to 150 % of the pixels: 4700x4348 per eye at Quality,
  vrcompositor.txt 2026-10-15). `pimax vr start` therefore sets SteamVR to manual 100 %
  (`supersampleManualOverride`, `supersampleScale` in steamvr.vrsettings, only while SteamVR is stopped), so the
  preset is the one setting.
- Without the variable Monado's default is 140 %; the launcher always passes it (100 by default).

## Frame health in the session card
While a session runs the card shows the render size and a line from `$XDG_RUNTIME_DIR/pimax-perf.stats`:
headset fps, **dropped frames** (`comp_skipped`: display periods where the headset showed the previous image again)
and the game's fps and GPU time against the frame budget (13.9 ms at 72 Hz).

`comp_late` is not a dropped frame: it counts compositor commits (render, present, then the bounded wait for the GPU
queue) that returned after the *predicted display time*. Monado's fake pacer gives the compositor only 3 ms before the
present and predicts display 4 ms after it, so the wait after presenting alone can pass that time. A smooth 72 fps
session showed 92 % late with 2 skipped in 14061 frames. The wrist readout (pimax-perf) counts only skipped frames as
missed since 2026-10-15; the card line shows late as a percentage ("92 % late") and the tooltip explains it.

## Other causes of delay (2026-10-15 findings)
- 6DoF: the SLAM gate dropped to 3DoF 43 times in a 3 minute session ("tracker drifting while still") and re-aligned
  with up to 74 mm / 2.8 deg corrections: position freezes and slides feel like lag.
- GPU sharing: Sunshine streaming (about 25 % of the GPU) and the local AI (llama-server on Vulkan) run beside VR.
- No async reprojection: SteamVR logs "Async support disabled ... direct mode driver"; Monado re-shows the last
  game frame with rotation-only timewarp. A game that misses its frame shows judder on position.

## Shaky desk screens (2026-10-16 18:08-18:16 Work session, 90 Hz)
- **Dropped frames, not tracking noise.** Monado showed 41644 frames while the passthrough app was paced 45638 times
  (8.8 % fewer); `comp_skipped` = 3920, about 8 per second (idle sessions: 0; a smooth 72 Hz one: 2 in 14061). A dropped
  frame repeats the image of 11 ms earlier: at an ordinary 15-60 deg/s head turn the screens jump 0.17-0.67 deg
  (6-25 panel pixels).
- **Tracking was fine.** Replaying the session's `head-imu.csv` through the driver's head tracker
  (`pimax-slam corrections`): jitter while worn and quiet 0.3-0.5 mm vertical, 0.8-0.9 mm horizontal (under one
  pixel on a screen 2.5 m away); IMU 1 kHz with no gap over 1.02 ms; 4 IMU samples held by the gate in the whole
  session; one tracker re-zero (about 18:13) held without a jump. Monado ran with the realtime GPU queue and SCHED_FIFO
  threads (cap_sys_nice was set).
- **The GPU was shared** with the local AI the whole time: llama-server (4B, Vulkan) answered 872 requests in those
  8 minutes (a voice audit run by an agent, ~110 per minute back to back, plus the user's 64 commands) and
  whisper-server (Vulkan) decoded ~290 utterances. NVIDIA time-slices between processes; earlier measurements give
  about one dropped frame per GPU model request (docs/AI.md). Sunshine took another ~30 % of the GPU.
- WayVR itself kept 90 fps except while loading an 8k backdrop (down to 386 frames in 5 s at 18:12:09); its layers
  are re-projected by Monado, so that is not the shake.
- What helps: no AI benchmarks on the GPU servers while VR runs; AI preset Low (model on the processor) or Parakeet
  (processor) speech recognition when smoothness matters; Sunshine off.
- **Timeline:** `pimax-perf` now writes one line every 5 s to the session's `perf.log`
  (`perf: unix=... mono_ms=... fps=... frames+N dropped+N late+N app_fps=... app_gpu_ms=...`), so the next session
  shows *when* frames dropped and they can be lined up with `voice.log`, `ai.log` and the llama-server log.

## Game mode: free the graphics card while playing
**Display** > **Performance**: one switch, "Game mode" (default on, also in Home > Quick settings). **Sunshine is never stopped or paused** (user rule; the old "Pause Sunshine" switch and `pause_sunshine` setting are gone, an old value in gamemode.toml is ignored and dropped at the next save). It only applies to **Play** (`pimax vr start`), never to Work.
- At Play start (after VR is up) it pauses the **local AI** (`pimax-ai stop`: SIGTERM,
  llama-server on Vulkan). Only what was really running is paused; it is written to
  `<session dir>/gamemode-paused.txt` (`ai=gpu|cpu`; a `sunshine=<unit>` line left by an older version is only started again, never written). The voice AI is not used in Play.
- Stop Play (`pimax vr stop`) starts exactly those again (`systemctl --user start`, `pimax-ai serve [--cpu]`, detached).
- Crash recovery: the app, at start and when no monado-service/vrserver runs, runs `pimax vr gamemode restore` if the
  state file is still there. `restore` refuses while a session runs.
- It is an optional step: any failure is a plain line in the log ("game mode: the local AI could not be paused (VR goes on)").
  The session card shows "Paused for game mode: local AI".
- CLI: `pimax vr gamemode [status|on|off|budget auto|6|8|restore]`; saved in `~/.config/pimax/gamemode.toml` (`pause_ai`, `comp_budget`;
  the old `game_mode` key is still read and means `pause_ai` only, so Sunshine is never paused).

## Compositor time budget
**Display** > **Performance** > **Compositor budget**: Auto / 6 ms / 8 ms (`comp_budget` in gamemode.toml,
`pimax vr gamemode budget auto|6|8`). It becomes `U_PACING_COMP_MIN_TIME_MS=6.0` / `8.0` for monado-service, set by the
one env builder (`vr/config.rs build_env`; a value already in the caller's environment wins). Checked in Monado
(`u_pacing_compositor_fake.c`: `DEBUG_GET_ONCE_FLOAT_OPTION(min_comp_time_ms, "U_PACING_COMP_MIN_TIME_MS", 3.0f)`):
a float in milliseconds, default 3.0 (Auto = not set). It is the time the compositor reserves before the display time,
so a bigger value starts each frame earlier: fewer late commits, a little more delay. SteamVR sessions get it too.

**A/B in plain words:** pick one choice, press Play (or Restart VR service), play the same scene for a minute and read
the session card line: *"headset 72 fps, 0.0 % dropped frames, N % late, game M fps"*. Then do the same with the other
choice. Keep the one with fewer dropped frames first, then lower late %, and a game fps that stays at 72. If nothing
changes, leave Auto.

## Brightness
**Display** > **Brightness**. Settings: `~/.config/pimax/display.toml`
(`image = "standard|bright|brighter"`, `backlight = 20..100`); CLI `pimax vr display [status|image X|backlight N|apply]`.

* **Panel backlight** (default 100 %). Measured: the bridge log prints `nvt_set_bl_brightness: 255` at every Linux
  panel-on, so the backlight was already at the maximum; the 211 seen in Windows sessions is lower. The panel keeps the
  last value it got (239 after a Windows session), so Play sends the saved value again after the session is up, and the
  slider sends it live. Packets exactly as in `20261013-124541-ipd-brightness-hub2.pcap` (analysis/controllers-settings.md
  section 2), 1 ms apart on the command channel: `58 0d 26 00 'pbrg' 00000000 01 PCT`, `58 04 2a 00 PCT`,
  `58 0b 27 00 'pbrg' 00000000`; unit tests compare the bytes with the capture. Panel value = floor(PCT * 255 / 100).
* **Image brightness** (default Bright). Two runtimes, two compositors:
  * Monado OpenXR: Monado's compositor makes the final image: `XRT_COMPOSITOR_OUTPUT_GAMMA` = 0.85 (Bright) or 0.75
    (Brighter), set by `build_env` at service start (a value already in the caller's environment wins; Standard = not set).
    The sRGB path passes game colours through unchanged (analysis/overnight/polish.md), so Standard is correct but not
    bright. A gamma below 1 lifts the mid tones (grey 128 shows as 142 / 156) while black and white stay: nothing clips.
  * SteamVR: vrcompositor (Direct mode) makes the final image and builds its own warp mesh; driver_monado only supplies
    devices, poses and distortion, so the gamma variable does nothing there. Play writes `steamvr.hmdDisplayColorGainR/G/B`
    (a key vrcompositor reads, default 1.0) = 1.12 (Bright) / 1.25 (Brighter) / 1.0 (Standard) into steamvr.vrsettings
    while SteamVR is stopped (other keys kept). A gain clips the brightest 10 to 20 % of the values, hence the small steps.
* The image level applies at the next Play or Restart VR service; the backlight applies immediately.
