# AI knowledge: working on this project (and on a user's machine)

For any AI agent (Claude Code or other) that changes this repo or helps a user run the Pimax Dream Air on Linux,
or that adds support for another headset, GPU, driver or compositor. Short on purpose: it points at the detailed
docs and records what went wrong before. The rules in `ai/CLAUDE.md` override anything here.

---

## 1. Read first

Read in this order. Each line says why.

1. `ai/CLAUDE.md`: the project rules (GUI first, definition of done, one whole build, safety). Binding.
2. `docs/LESSONS.md`: 52 numbered pitfalls with the rule each one became (42-50: the reinstall day 2026-10-17, docs/RETRO-2026-10-17.html; 51-52: gaze and screen layout). Read all of it before touching the stack.
3. `docs/INVENTORY.md`: every component, command, device path and past problem with its commit.
4. `docs/ARCHITECTURE.md`: the layers (app, core, device modules, doctor) and the open-source design. Scope is the Pimax family.
5. `docs/HEADSET-COMMANDS.md`: the HID bytes the Windows captures verified (IPD, restart, controller touch byte).
6. `docs/STEAMVR.md`: the SteamVR path (driver_monado, Touch profile, haptics, runtime files, the live-test fixes).
7. `docs/ROOM-SETUP.md`: floor, play area, centre and forward. What the user does and the engine behind it.
8. `docs/RENDER-QUALITY.md`: quality presets, frame health, game mode, compositor time budget, brightness.
9. `docs/AUDIO.md`: how sound reaches the headset (DisplayPort audio, not USB).
10. `docs/HANDS.md`: camera hand tracking (Mercury models, licence limits, install steps).
11. `docs/VOICE.md` and `docs/AI.md`: the voice commands, the wake word, the local and remote AI routes.
12. `analysis/windows-capture-20261015.md`: the newest Windows capture, with the Windows clock offset.
13. `analysis/slam-drift.md` and `analysis/tracking-vs-windows.md`: why 6DoF drifts, what the still gate and the fused output do, how we compare with Pimax.
14. `analysis/edid-non-desktop.md` and `analysis/srgb-frozen-debug.md`: how the headset is presented to the GPU, and the dark/frozen image.
15. `analysis/hang-worn-debug.md` and `analysis/init-sequence.md`: the worn freeze, and the cold-start command sequence from Windows.
16. `analysis/controllers-settings.md`: IPD and settings commands, controller report layout.
17. `analysis/room-map.md`, `analysis/crashes-2026-10-15.md`: room relocalisation limits; crash triage that was done live.
18. `git log --oneline | head -200`: the commit subjects are detailed. Search them before you debug something old.
19. Other `analysis/*.md`: skim the titles (`head -3 analysis/*.md`) and open the one that matches your problem.

---

## 2. The system in 15 lines

1. **omarchyvr-app** (the "Pimax" app, `rust/omarchyvr-app`, core in `rust/omarchyvr-gui-core`): the only thing the user clicks. Play, Work, Quality, Room, Checks, Autofix, Logs, AI tab. It only has switches and calls the tools.
2. **pimax** CLI (`rust/omarchyvr-tools`, installed as `~/.local/bin/omarchyvr`): the launcher and engine behind the app (`vr start|stop|reset-headset|games|steam-setup`, `desk`, `controllers`, `ipd`, `audio`, `gamemode`). Agents and tests use it directly.
3. **libpimax / pimax-proto / pimax** (`rust/pimax*`, C ABI in `pimax-capi`): hidraw I/O and framing for the headset (34a4:0044), the bridge (34a4:0043) and the camera (34a4:0014). Commands come only from Windows captures and the allowlist in `rust/pimax-device/devices/<id>/device.toml`.
4. **omarchyvr-keepalived** (user systemd service, always on): wake and 213 ms keepalive, drains the interfaces. Without it the headset sleeps.
5. **pimax-cam broker** (`rust/pimax-cam`): the only process that opens the cameras. Everything else (6DoF, hands, controllers, passthrough, recording) reads frames from it.
6. **Monado fork** (`~/.local/lib/omarchyvr/monado`, branch `desk`; one build): the OpenXR runtime. It carries the Pimax driver, lenses, controllers, eye gaze, SLAM (6DoF via Basalt, behind `PIMAX_SLAM`), camera hands, the room anchor and the perf stats. One build, markers checked.
7. **SLAM and fused output**: Basalt on the camera and IMU streams, a still gate, alpha-beta fusion of the tracker into the IMU prediction (`PIMAX_SLAM_FUSE`), correction decay at rest. Output is a head pose in Monado.
8. **driver_monado for SteamVR** (`steamvr-monado/`, built with Monado): SteamVR's vrserver loads it for tracking and Touch controllers. Games choose it with `omarchyvr vr games steamvr` or use xrizer with `monado`.
9. **xrizer** (`~/src/xrizer`): OpenVR-to-OpenXR layer for Steam games in the default SteamVR-less mode. Its log is `~/.local/state/xrizer/xrizer.txt`.
10. **WayVR desk** (`~/src/wayvr`, branch `eye-gaze`): overlay screens (up to 4 Hyprland outputs), gaze pointer, hands from XR_EXT_hand_tracking, pinch. Started by `desk start`; the perf panel and captions run on it too.
11. **Room map** (`omarchyvr-roommap`, `omarchyvr-anchor`, `omarchyvr-room`): persistent map from a 6DoF session (ORB keyframes, loop closure) or an AprilTag on the desk; writes the Monado world offset (`PIMAX_ANCHOR`). Floor detection in `omarchyvr-roommap`.
12. **Eye tracker** (`pimax-eye`, `pimax-eyecal`): Tobii EyeChip (2104:0561). Streams gaze to Monado (`PIMAX_EYE=1`). Only one process may open it.
13. **Voice and AI** (`omarchyvr-voice`, `omarchyvr-voice-hud`, `omarchyvr-ai`, `omarchyvr-config` ai.toml): local STT (Parakeet via sherpa-onnx), wake word "Max", intent catalogue, optional local llama.cpp model and remote providers in two stages. Typing goes to the focused app only.
14. **omarchyvrd and omarchyvr-components** (`rust/omarchyvr-components`, `docs/PIMAXD.md`, `docs/RESILIENCE-PLAN.md`): the component graph that restarts leaves and core parts within budgets, with a GPU gate and a watchdog. Optional parts are leaves; VR keeps running when a leaf fails.
15. **Windows capture tooling** (`windows/capture-session.cmd|ps1`, `windows/usbpcap-capture.ps1`, `docs/WINDOWS-CAPTURE-PLAN.md`): USBPcap on the Windows PC running Pimax Play, read-only collection, decoded offline with `pimax pcap` and `omarchyvr-slam bench-windows`. This is the only source of new HID bytes.

---

## 3. Hard rules

These are the safety and process rules. Copy them into your plan. If a task needs one broken, stop and ask the user.

- **GUI first.** The user does not use the command line. Never answer the user with a command to run. Add a switch, button or page in `omarchyvr-app`, and make it call the tool. Root steps go through a pkexec prompt from the app.
- **Every feature is a switch in the app**, saved in the config and shared with the CLI, shown with its status and its errors in plain words, and recorded in the session's start argv so restarts keep it.
- **One whole build.** Every feature is compiled into the one Monado build and the one Rust workspace. Only settings turn features on. No feature gets its own build dir or a flag-only CLI path.
- **Required Monado markers** in `monado-service` (`grep -ac`): `PIMAX_SLAM`, `PIMAX_ANCHOR`, `PIMAX_EYE`, `PIMAX_CTRL_TRACK`, `held from the tracker` (IMU gate), plus each new feature's marker. The app's preflight (`omarchyvr-gui-core` builds.rs / preflight.rs) refuses a build without them.
- **Monado cache options** are copied from the live build, never from a fresh cache: `XRT_BUILD_DRIVER_PIMAX=ON`, `PIMAX_CAM=ON`, `XRT_HAVE_SYSTEM_CJSON=OFF` (+ ONNX Runtime for hands).
- **cap_sys_nice after every relink.** `getcap <build>/src/xrt/targets/service/monado-service` must show `cap_sys_nice=ep`. The app's Fix or Play button restores it with pkexec.
- **Rust:** build the whole workspace from a clean worktree of committed HEAD (`CARGO_TARGET_DIR=rust/target-clean`), then install all binaries together. No half-installed sets.
- **HID bytes only from Windows captures** (USBPcap, Pimax Play running) and only if they are in the allowlist in `device.toml`. The GUI never sends HID itself. It calls the tools. Approved commands so far: camera exposure and LED frames `58 19 51 ...` (Windows ranges only), IPD motor `58 06 5c 05 01 <mm*100 LE>`, StartPair `58 04 04 00 SS` (once per user request), haptics `58 09 07 SS AMP DUR`, unpair `58 03 05 SS`, restart `58 04 28 00 00`. Not approved: device-side eye calibration ops, `58 03 19 00`.
- **No firmware flashing.** No DRM hotplug. Never force a display hotplug.
- **Never tell a user to unplug the headset** (one exception: the last-resort power-cable cycle (in the setup or in the wake guide after a failed Work/Play start) when `34a4:0044` stays off, no session running, after the power button failed; LESSONS 40). Recovery is `omarchyvr vr reset-headset` (Restart headset), then the camera auto-recover, then a reboot as the last step (LESSONS 26, 33).
- **No sudo by agents.** Root is run by the user, from the app, through pkexec. Don't edit hypr, systemd system units, NVIDIA or boot config, or `~/.config/wayvr`.
- **Never touch a live VR session.** Install and build only between sessions. Stop processes by PID only (`omarchyvr vr stop --grace 30`), never `pkill -f`. Never send keys or typing to the user's windows.
- **Heavy work** (builds, tests, benches) runs at `nice -n 19` with `-j8`.
- **Local commits only** for the forks (Monado, WayVR). Never push them. For omarchyvr: commit only your own files and push origin main.
- **Secrets:** no API keys, tokens, emails or device serials in the repo. Keys go in files the user owns (`~/.config/omarchyvr/llm.key`), never in code.
- **Mercury hand models** have no licence for redistribution: personal use only.
- **Tests must not touch user config:** set `HOME` to a temp dir in any test that can reach `~/.config` (LESSONS 25).
- **Voice commands** run without confirmation by default (`voice_confirm = none|dangerous|always`); the lock-screen and close-window actions are careful tools.
- **Install only from a clean build of committed HEAD**, between sessions, by the main thread.

---

## 4. Pitfalls we hit

Symptom, cause, fix and where it is recorded. "L n" is `docs/LESSONS.md` row n.

### 4a. Steam, SteamVR and games

| Symptom | Cause | Fix | Where |
|---|---|---|---|
| Steam VR game: `XR_ERROR_RUNTIME_UNAVAILABLE`, or Monado client: `Failed to connect to socket .../monado_comp_ipc` | Steam Linux Runtime's pressure-vessel (0.20260925) shares the host OpenXR runtime and the IPC socket only with the env var | Launch option `PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1 %command%`; the app's "Steam set up for VR games" (`omarchyvr vr steam-setup on`) | 7f3fbf2, 1f2890c; L23, L27 |
| Games still fail after the launch option works | `XR_RUNTIME_JSON` alone is not enough | Keep the runtime link pointing at the started build (the start does it, `--no-set-runtime` to skip) | 2910177; L24 |
| Dungeons of Eternity: `XR_ERROR_RUNTIME_UNAVAILABLE` in both modes | The container did not get the socket (L27), not wineopenxr | Same fix as L27; OpenVR mode (xrizer) stays the default | L23 |
| SteamVR exits with "[PROBLEM] Saving Path Registry failed" (shown as a crash) | `~/.config/openvr/openvrpaths.vrpath` is read-only and SteamVR wants to write it | "Games run with" owns the file: `omarchyvr vr games steamvr` (writable) or `monado` (xrizer, 0444) | fae5039, 62ce778; L28 |
| SteamVR "vrserver" rewrites `openvrpaths.vrpath` on every start | SteamVR's own install writes its copy over xrizer's | `vr start`/`desk start` put xrizer back (SteamVR copy kept as `.pimax-steamvr`) | L27 |
| `vrmonitor` aborts at start (Qt wayland) | Omarchy sets `QT_QPA_PLATFORM=wayland;xcb`, SteamVR's bundled Qt has only xcb | The launch sets `QT_QPA_PLATFORM=xcb`; check with `tr '\0' '\n' < /proc/$(pgrep -x vrmonitor)/environ` | 4d43857; STEAMVR.md, analysis/crashes-2026-10-15.md section 7 |
| `vrcompositor` SIGKILLed about 1 s after start | vrserver sets `RLIMIT_RTTIME` to 0; the realtime watchdog kills a process that runs with rttime 0 | `steamvr-monado/bin/linux64/pimax_rttime_preload.so` in `LD_PRELOAD` (makes the RLIMIT_RTTIME calls a no-op); `vr start` sets it | 1ec3972; STEAMVR.md section 2 |
| SteamVR runs at 90 Hz on a 72 Hz panel (or the picture judders) | SteamVR chooses its mode from `steamvr.preferredRefreshRate` (default 90), ignores the driver's rate list | Write `preferredRefreshRate` = `PIMAX_FPS` into `~/.local/share/Steam/config/steamvr.vrsettings`, keeping other keys | STEAMVR.md section 2 |
| Head moves twice as far as the user moves (neck model on top of SLAM) | SteamVR adds its neck model when `position_tracking=false`; under 6DoF the head is already tracked | With 6DoF the driver sets `shouldApplyHeadModel = false`, `willDriftInYaw = false`; check vrserver.txt for `head model OFF` | cbe757b; L30 |
| SteamVR buttons "not mapped" | Monado's generated `oculus_touch` profile was incomplete (trigger without value, menu on the wrong path, no grip touch) | Register SteamVR's own Touch profile (`{oculus}/input/touch_profile.json`) and its paths; check the `Added input` list in vrserver.txt | cbe757b; L30 |
| Controller vibration does nothing | No handler for cmd 0x07; the first capture showed only other commands | Haptics only from the captured bytes `58 09 07 SS AMP DUR`; never AMP 0; one command per 8 ms per hand | 288b409; L32 |
| Game mode stopped game streaming (Sunshine) every time | Game mode paused Sunshine by default | `pause_sunshine` is off by default; only the local AI is paused by default; the old `game_mode` key means `pause_ai` only | 6ebf831, 02adae9; RENDER-QUALITY.md |
| Dark and frozen image in a game, DP link fine | Either the wineopenxr/D3D11 hand-off (dumps dark) or the display path (dumps bright, display engine timeouts) | Compare the compositor dumps with the headset: dark dumps = client hand-off; bright dumps = nvidia/DP path (check `Idling display engine timed out` in the kernel log) | analysis/srgb-frozen-debug.md |
| Game looks too dark only with sRGB swapchain | 10-bit target has no sRGB encode | Brightness switch (Standard/Bright/Brighter) sets the output gamma; tune only if the game looks too dark | a23b335, 42761f9, a1d7ff9; srgb-frozen-debug.md |
| Steam VR game shares the GPU with the local AI and Sunshine: stutter | GPU sharing (AI on Vulkan, Sunshine ~25 % of the GPU) | Game mode pauses the local AI on Play; compositor time budget; perf readout | RENDER-QUALITY.md; 02adae9 |

### 4b. Monado, GPU and display

| Symptom | Cause | Fix | Where |
|---|---|---|---|
| `monado-service is already running` on a second start | The previous run was still exiting | Wait until `pgrep -x monado-service` is empty; stop with `omarchyvr vr stop --grace 30` | L1 |
| Camera "Device or resource busy", no 6DoF | Only one process may stream the cameras; a stalled run held it | `pimax-cam broker`; every consumer uses `--source broker` / `PIMAX_CAM_SOURCE=broker` | L2, d2bdb13 |
| Eye tracker cannot be opened | One process per EyeChip | Run `pimax-eye watch`/`ipd` without `--eye` (or without Monado) | L3 |
| Headset black although the app is focused | Monado started without the launcher's compositor env | One env builder (`rust/omarchyvr-tools/src/vr/config.rs`); never hand-roll it | L5 |
| Pose printer or helper took the display | Monado shows the last main app | Only one main OpenXR app; helpers are `XR_EXTX_overlay` sessions (`POSECLI_OVERLAY=1`) | L4, f7cd5f4 |
| 6DoF ran 3DoF only: `built without -DPIMAX_CAM=ON` | Build configured from scratch with `PIMAX_CAM=OFF`; string checks passed | Check `PIMAX_CAM:BOOL=ON` in `CMakeCache.txt`; rebuild with `cmake -DPIMAX_CAM=ON .` then ninja | L6, f568424 |
| Build for the app: 6DoF blocked by preflight | build-rec was configured with the Pimax driver OFF (2026-10-15) | Copy the cache options from the live build, check the markers before switching `~/.local/lib/omarchyvr/monado` | ai/CLAUDE.md section 3 |
| Monado stutters or fails the priority request after a rebuild | Every relink drops `cap_sys_nice` | Fix/Play button (pkexec) or `setcap cap_sys_nice+ep` by the user; check with `getcap` | L7, f568424 |
| Replacing the launcher made the next start fail | Installing the binary while desk mode runs | Install only when nothing runs | L8 |
| Xid 31 from WayVR refuses every start for the boot | Per-app MMU fault the GPU recovers from | Xid 31 from wayvr = warning + "start anyway"; other Xids = red, reboot. Never run WayVR headless renders from the GUI | L9 |
| Headset image freezes after about 20 s when worn; kernel log quiet | Hang in the flip path (NVIDIA) | `merged_head_sync=1` and the 3 DKMS patches; health checks in L10; analysis/hang-worn-debug.md | L10 |
| Monado ignores SIGINT at stop and is SIGKILLed | Most likely a stuck GPU flip (not proven) | `omarchyvr vr stop` ends a hung WayVR once Monado is gone; flip safety in the Monado branch | 1abebff; hang-worn-debug.md |
| Headset gone from USB after a long session ("USB disconnect", camera 34a4:0014 missing, HID stays) | The USB3 side dropped | `omarchyvr vr reset-headset` (Restart headset, `--mode sys` by default); `desk start` auto-recovers the camera; if still gone, `--mode dp`, then a reboot. Never a physical unplug | L26, b240ca7, f9ff672 |
| Restart headset took about 90 s | It replayed Pimax Play's exit frames and waited for the firmware heartbeat | The real button is `58 04 28 00 00` (SysReset): HID back ~8 s, camera ~17 s, picture ~22 s | L33, 38d3405 |
| First wake after a restart is ignored | Firmware ignores the first wake | The session re-sends the wake once after 0.5 s without a `4c .. 1a` ack | L33 |
| Touch byte bit 0x10 read as a button or rest state | Bit 0x10 means "hand on controller" on both hands | `TouchBits::t4` is `hand_on`; no left/right rest value | L33 |
| Shared drive does not mount | Needs ntfs3 uid/gid | `omarchyvr mount-shared` (root via the app) | L21 |
| Dungeons of Eternity or any Monado build: libcjson missing in the container | Monado used the system cJSON | `XRT_HAVE_SYSTEM_CJSON=OFF` (bundled cJSON) | INVENTORY history 2026-10-13; 414852f (Monado) |
| Monado client won't see a runtime it used yesterday | `~/.config/openxr/1/active_runtime.json` pointed at a stale build | `vr start`/`desk start` repoint it atomically (one-time `.pimax-backup`) | L24, 2910177 |
| A test changed the real OpenXR runtime link | The test inherited the real HOME | Temp HOME in every test that can reach user config | L25 |

### 4c. 6DoF, SLAM and room

| Symptom | Cause | Fix | Where |
|---|---|---|---|
| Metres of drift with the headset on a cushion | Cameras see fabric; IMU-only dead reckoning with a zero gyro-bias seed | Seed the gyro bias, Pimax noise values, still-hold with hysteresis (`PIMAX_SLAM_STILL=0` disables). Tell the user: lit room, cameras free | L13, 2266f74 |
| 43 "6DoF: lost (tracker drifting while still)" logs while lying down; swing in the view | The still gate re-zeroes a landmark-free tracker and each reset was reported as a loss | A still reset is a re-zero (output held, no loss). Check the tracker attitude at each reset before tuning. `omarchyvr-slam stillgate DIR` replays it | L29, 50d6a8b; analysis/slam-drift.md |
| Sliding and up-and-down at rest while playing | Every 25 Hz tracker pose replaced the prediction base; noisy velocity became 20-150 mm steps | Alpha-beta fusion (`PIMAX_SLAM_FUSE`), slow decay at rest (`PIMAX_SLAM_TAU_REST_MS`). Measure with `omarchyvr-slam corrections DIR` and the live `6DoF correction:` lines | L31, 32b9386 |
| AE/exposure change froze cameras and IMU | Basalt's IMU queue filled during the 0.5 s camera restart and deadlocked | Gate the IMU fed to the tracker (fixed in Monado `slam` d3138d4). AE is safe only with the fixed builds | L12, d3138d4 |
| Room map refuses to relocalise in the dark | ORB has no features in a dark room (test clip mean 6/255); the tool refuses and leaves the offset alone | Map once per lighting condition (two maps, try both). Lamps on vs off changes the features | analysis/room-map.md section on lighting |
| Room map off by 15-81 mm between sessions | Expected same-place error (median 15-81 mm over 7 pairs) | Not a bug; the anchor is for the WayVR windows, not for precision | INVENTORY omarchyvr-roommap; room-map.md |
| Headset tracking worse than Pimax Play on Windows | Pimax zero lag, no smoothing, 90 Hz state with extrapolation; loses 6DoF in fast turns | Our output matches the Windows log per the bench; no live default changed | d33316e; analysis/tracking-vs-windows.md |
| Controllers swing or jump with the optical pose | LED-based poses are noisy and drop out | Alpha-beta by LED count, gyro-carried orientation, stronger at rest; hands fused from the gyro when out of view | 648ec32, 7517b56 |

### 4d. Eye, audio, voice and controllers

| Symptom | Cause | Fix | Where |
|---|---|---|---|
| Gaze off by ~6 deg per mm of error in the eye origin; right eye jumps | Origin jumps in a bad state; direction is garbage | Fit per eye with an origin term; drop eyes > 5 mm from their usual origin; validation 3.08 -> 2.38 deg (14 Oct) | L22, 4af3c8f |
| Eye calibration still off 2 deg after a while | Calibration and validation phases drift | Interleave validation and repeat the centre target | L22 |
| Headset microphone sends zeros | The mic is silent while the panel is off | Report all-zero input as a `no_device` status; turn the panel on first | L19, 97d3891 |
| No sound in the headset | Headset audio is DisplayPort audio (DPCD: audio capable, EDID CTA block); USB is only the mic | Use only a real headset sink (`omarchyvr vr audio on|off`); never fall back to the projector or other hardware | AUDIO.md; 19ae864, 11c5345 |
| Voice AI: every request answers HTTP 400 "Undefined rule identifier 'side'" | A new slot type had no GBNF rule in the grammar | Every grammar rule must be defined; a test checks it. Errors show the server's body | docs/AI.md section 15 |
| Voice AI misreads "dark" or "background" as a preset | Local 4B model; the answer format placeholders were copied | Routing and context rules in docs/AI.md sections 3-4 | docs/AI.md sections 4, 11 |
| "Window closed" when a mode hid the screens | Modes hid the VR screens, which looked like closed apps | Modes never make apps vanish; layout by kind inside a screen | bbc9ac1 |
| Voice wake fails on short utterances ("I") | Lead window longer than the utterance | Fixed: wake() handles short utterances | 5badc1e |
| Typing goes to the wrong app or garbled keymap | Every new virtual keyboard switched Hyprland's keymap | One persistent virtual keyboard with one fixed keymap per process | docs/VOICE.md typing section |
| Left controller "sleeping" 3 s after its report, gone for 25 min; pairing hangs | The left controller sleeps; no unpair was ever sent for it | Pairing and unpair refuse while a session owns the headset (button disabled with the reason). Left unpair is enabled only with the capture-verified bytes (`58 03 05 SS`); StartPair is approved once per user request | 60f426c, 46b6424, 5254712; analysis/windows-capture-20261015.md |
| Power button on the headset: panels go off, status light stays green | It only turns the panels off; it never turns the headset off | Do not tell the user to power-cycle in chat; use the app's Restart headset or Play. A control chip that stays off, or cameras that do not come back after one restart: the app's "Get the headset ready" guide (the power button first, the power cable only as its last step, never during a session) | 049feaa; L40, L43 |

### 4e. Windows capture tooling

| Symptom | Cause | Fix | Where |
|---|---|---|---|
| Windows timestamps 8 h off from Linux | The Windows PC clock is 8 h early on that machine | Use the same offset for every Windows file (they share the clock); say so in the notes | analysis/windows-capture-20261015.md header |
| Camera iso packets cut at 64 KB | USBPcap truncates camera URBs at about 64 KB | Snaplen 65535 for camera and tracking takes (0.2-0.7 GB each); 64 KB is still enough for frame counters and timestamps | docs/WINDOWS-CAPTURE-PLAN.md |
| Capture shows keystrokes from the keyboard | The USB keyboard sits on the same hub (hub2) as the headset | Never type a password during a capture; the capture session warns | analysis/windows-capture-20261015.md section 9 |
| Bluetooth controllers' traffic missing from PC captures | Controllers talk to the headset radio, not the PC's Bluetooth | Nothing to capture on the PC; a Bluetooth scenario is dropped | windows-capture-20261015.md table |
| Capture takes unreadable with a single hub | Everything real is on hub2; hub1 is empty | One USBPcap process per root hub, every hub | docs/WINDOWS-CAPTURE-PLAN.md |
| Old "unplug the junction box" advice | Wrong: the camera came back by itself ~90 s after a reset | Never ask for an unplug; see Restart headset | L26 |

---

## 5. How to add support for a new headset, GPU, driver or compositor

Start from the rules in section 3. Then work in this order. Do not skip the capture.

**New headset (same vendor, or another one)**
1. Add a descriptor: `rust/pimax-device/devices/<id>/device.toml`, with the USB IDs and the HID allowlist. A module is matched only by its USB IDs. The Pimax family is the scope (ARCHITECTURE.md); the Dream Air SE, Crystal Light, Crystal Super and 8KX are mocks until captured.
2. Capture on Windows first, with the vendor's app running. Use `windows/capture-session.cmd` (guided, Administrator) or `windows/usbpcap-capture.ps1`. One USBPcap process per root hub; snaplen 65535 for camera and tracking; no typing; write `notes.txt`. Copy the pcaps to `pimax-data/` (not committed).
3. Decode offline: `pimax pcap <file> --csv out.csv --head 50` and the analysis tools. Pair each command with the vendor's visible action (note the time). Record the bytes and the reply in a `docs/*-COMMANDS.md` file, marked verified or guessed, as `docs/HEADSET-COMMANDS.md` does.
4. Only verified commands go in the allowlist. Anything else stays unsent. Ask the user before adding a new kind of command (device-side calibration, firmware and eye-calibration writes are not approved).
5. Add the device module checks in `rust/pimax-device/src/checks/` and the fixes in the Fix catalogue (fix text is shown to the user; the Gate sends only allowlisted bytes).
6. Tests: unit tests on the decoders with the captured bytes as fixtures (`rust/pimax-proto`, `rust/pimax-device`), a gate test that refuses a non-allowlisted byte, and `cargo test --workspace`. The app preflight test must pass with the new descriptor.
7. Show it in the app: a card or page, status and errors in plain words, a switch if it is optional, and a screenshot (`omarchyvr-app --screenshots DIR`) that you actually look at.

**New GPU or driver (NVIDIA or other)**
1. Read `docs/LESSONS.md` rows 9-10, `analysis/nvidia-flip-hang.md`, `analysis/nvkms-mode-validation.md`, `analysis/nvidia-dsc-linkrate.md`.
2. Health checks belong in `omarchyvr-doctor` (driver version, module parameters, patched module presence, Xid in the kernel log). Fix plans are printed, never run by the tool.
3. Test: `omarchyvr-doctor` must report the state on the new machine; run a short desk start with the app's preflight and read the Xid list.

**New compositor or desktop (not Hyprland)**
1. The launcher sets the Wayland direct-mode env in one place (`rust/omarchyvr-tools/src/vr/config.rs`). Add the compositor's connector and lease handling there, and keep `XRT_COMPOSITOR_FORCE_WAYLAND_DIRECT`.
2. WayVR config and the screen layout are in `docs/SCREENS.md` and `wayvrctl`. Test with `desk start` and `omarchyvr desk screens`.
3. Voice window control uses Hyprland (`win.*`, `omarchy.*`). A new compositor needs its own adapter; the intent catalogue stays the same.

**New SteamVR or game path**
1. Read `docs/STEAMVR.md` and `docs/GAMES-CHECK.md`. Run `omarchyvr vr game-check` (exit 1 on any FAIL) with the game closed, then with it open.
2. Launch options are shown in the app, never set silently. Runtime files: `openvrpaths.vrpath`, `active_runtime.json` (LESSONS 24, 27, 28).

**Markers and tests before a build is accepted**
- Every new feature adds a marker string to `monado-service` and a line to the app's preflight (builds.rs).
- Goldens: `omarchyvr vr start|stop --dry-run` goldens and the app's screenshots must match after a change (`cargo test --workspace`).
- Install only from a clean worktree build of committed HEAD, between sessions, after `getcap` shows `cap_sys_nice=ep`.

---

## 6. How to diagnose a user's failure

Start with the app's Checks page (its preflight is the same code). Then the logs below. Run read-only checks first.

| What | Where | What a line means |
|---|---|---|
| Session logs (desk/VR) | `/tmp/omarchyvr-session/monado.log` (also `passthrough.log`, `wayvr.log`, `perf.log`, `broker.log`, `ctrltrack.log`) | Monado: `Startup Complete` = compositor up; `6DoF: lost (...)` = tracker lost (read the reason: still gate, drift, camera); `built without -DPIMAX_CAM=ON` = wrong build (L6); `monado-service is already running` = previous run not gone (L1) |
| Previous session | `/tmp/omarchyvr-session/monado.log.prev` | the run before the last one; use it after a restart |
| 6DoF test runs | `~/.local/share/omarchyvr/slam-recordings/<stamp>-live/monado.log` | Per-run recording with the EuRoC data; `omarchyvr-slam stillgate|corrections DIR` replay it |
| Pose printer and camera views | `/tmp/pimax-slamview.*` | helper overlay sessions; not the main app |
| SteamVR | `~/.local/share/Steam/logs/vrserver.txt`, `vrcompositor.txt`, `vrmonitor.txt` | `Added input` list (controllers), `head model OFF` (6DoF), `[PROBLEM] Saving Path Registry failed` (read-only openvrpaths, L28), `Idling`/`rttime` kills |
| xrizer (SteamVR-mode games) | `~/.local/state/xrizer/xrizer.txt` | OpenVR-to-OpenXR errors; `XR_ERROR_RUNTIME_UNAVAILABLE` here means the container did not get the socket (L27) |
| Game | Unity `Player.log` in the game's Proton prefix (`AppData/LocalLow/...`); Steam `logs/` | Engine errors, OpenXR session state |
| VR games check | `omarchyvr vr game-check [--appid N] [--json] [--no-container]` | Runtime files, Monado, a tiny OpenXR client inside Steam's container with and without the env var; exit 1 on FAIL |
| Doctor | `omarchyvr-doctor` | Read-only: USB/udev, keepalive, NVIDIA patches (viewport, flip-fix, bounded-wait), EDID connector, lease, setcap, kernel log |
| GPU | `journalctl -k -b | grep -i -E 'NVRM|Xid|nvidia-modeset'` | `Xid 31` from wayvr = per-app (warning); other Xid, or `Idling display engine timed out` = display engine hang, reboot (L9, L10) |
| USB | `journalctl -k -b | grep -i 'usb 2-'` (headset on bus 2 hub) | `USB disconnect` with HID present = USB3 side dropped; use Restart headset (L26) |
| Camera | `ls /sys/class/video4linux`, `fuser /dev/video*` | Missing camera = USB side dropped; a busy camera = a second consumer (L2) |
| Keepalive | `systemctl --user status omarchyvr-keepalived` | Must be running; restart it, do not stop it by hand |
| Audio | `pactl list sinks short` | The headset sink must be DisplayPort audio; none = panel off or no DP audio (AUDIO.md) |
| Voice | `omarchyvr-voice` logs and the debug line under the captions (VOICE.md section 12); `omarchyvr-voice audit` | `HTTP 400` = grammar bug (AI.md section 15); `no_device` = mic silent (L19) |
| Crashes | `coredumpctl list`, the `diagnose-crash` skill | SIGSEGV or SIGABRT with a core; see analysis/crashes-2026-10-15.md. Four "segfault at 0" lines 0.9 s apart named `ovr-test-crash` (before 2026-10-17: `omarchyvr-app`) with no core are the supervisor test's crash on purpose, not the app (L46) |
| Setup and install runs | `~/.local/state/omarchyvr/jobs/*.log` (file names in UTC), `~/.local/state/omarchyvr/logs/omarchyvr-app.log`, `~/omarchyvr-install-*.log`, `journalctl --user -u omarchyvr-keepalived` | Count the pkexec prompts (`journalctl -b \| grep pkexec`) and the reboots of a run; "waiting for headset 34a4:0044" for long = the headset's control chip is off (L41, the guide); "headset gone after N s" then "headset on /dev/hidrawN" = a re-enumeration the keepalive handled |
| Headset parts on USB | `journalctl -k -b \| grep -E 'idVendor=(34a4\|2104)\|USB disconnect'` | `34a4:0044` re-enumerating every ~5 s = no host heartbeat (no keepalive access); `2104:0561` gone after a headset restart = normal (eye module off until the session's enable, L44); `34a4:0014` with its hub `05e3:0620` gone = USB3 side off (one restart, then a power cycle, L43) |
| Setcap | `getcap <build>/src/xrt/targets/service/monado-service` | `cap_sys_nice=ep` must be present; without it the app's Fix button (pkexec) restores it (L7) |
| Build markers | `grep -ac PIMAX_SLAM <build>/.../monado-service` (and the other markers) | 0 = the build lost that feature; preflight refuses it (CLAUDE.md section 3) |
| Runtime link | `cat ~/.config/openxr/1/active_runtime.json` | Must point at the build in use (L24) |

When you report a failure to the user, give the symptom in plain words and the fix as a button or a switch in the app. Never a command to type (CLAUDE.md section 1).

---

## 7. Tested on

Facts checked on the project PC, read-only, 2026-10-15. Other machines will differ; the checks in section 6 tell you which.

- Hardware: Pimax Dream Air (one unit), Intel Core i9-14900KF (32 threads), 62 GB RAM, NVIDIA GeForce RTX 4090 (AD102).
- GPU driver: NVIDIA open kernel module 610.57.04 (with the three pimax DKMS patches and `merged_head_sync=1`).
- OS: Omarchy 4.0.4 (Arch-based), kernel 7.2.5-3-omarchy.
- Compositor: Hyprland 0.56.2 (Wayland, DP-1 for the headset; direct mode via DRM lease).
- SteamVR 2.17.10 with the Monado driver (`driver_monado.so`), Steam Linux Runtime pressure-vessel 0.20260925.
- Windows PC for captures: Pimax Play and USBPcap 1.5.4 (clock 8 h early).
- Not tested here: AMD or Intel GPUs, other compositors (KDE, GNOME, X11), other headsets, other Steam Linux runtimes.
