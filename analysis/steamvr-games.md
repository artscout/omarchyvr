# Running SteamVR (OpenVR) games on the Dream Air: xrizer + Monado

Status (2026-10-13): Steam, SteamVR, Proton are NOT installed. xrizer is built and smoke-tested
(OpenVR client -> xrizer -> Monado, windowed). No game has been run yet.

## Stack
OpenVR game -> xrizer (`libxrizer.so` as OpenVR runtime) -> OpenXR loader -> Monado (`~/src/monado/build`, pimax driver) -> direct mode on the headset.

## What was done
- `sudo pacman -S rust` (rust 1.98.1, cargo). Nothing else installed.
- `~/src/xrizer` cloned from github.com/Supreeeme/xrizer, built with `cargo xbuild --release` (64-bit only).
  Runtime dir: `~/src/xrizer/target/release` (contains `bin/linux64/vrclient.so`, `libxrizer.so`).
  32-bit build skipped (only for old 32-bit games; needs `rustup target add i686-unknown-linux-gnu`, not available with the Arch `rust` package; would need rustup).
- `~/src/openvr` (Valve SDK, cloned + built `libopenvr_api.so`) used only for the smoke test.
- `analysis/tools/ovr_smoke.cpp` + binary `ovr_smoke`: VR_Init + IVRSystem queries.
- Config written by `analysis/tools/vr_runtime_switch.sh xrizer`:
  - `~/.config/openvr/openvrpaths.vrpath` -> xrizer runtime dir (did not exist before)
  - `~/.config/openxr/1/active_runtime.json` -> symlink to `~/src/monado/build/openxr_monado-dev.json` (did not exist before)
  - originals (here: "absent") recorded in `~/.config/pimax-vr-backup/`.

## Revert
`analysis/tools/vr_runtime_switch.sh steamvr` restores the saved files (or deletes ours if none existed).
`vr_runtime_switch.sh status` shows the current state. Once SteamVR is installed and run, it rewrites openvrpaths.vrpath itself;
run `vr_runtime_switch.sh xrizer` again to point back at xrizer (the backup is only taken on the first switch).

## Smoke test result
Windowed Monado (`XRT_COMPOSITOR_FORCE_XCB=1 XRT_COMPOSITOR_SCALE_PERCENTAGE=30`, `sleep 40 | monado-service`), then `tools/ovr_smoke`:
VR_Init OK, xrizer created an OpenXR instance + Vulkan session ("Pimax Dream Air" head selected, left/right controllers present),
recommended target 1152x1065 at 30% scale, projection matrix sane, HMD connected.
Findings:
- First attempt failed: `Monado client library version 13d4703 does not match service version cd69718`. The dev build has mismatched
  libopenxr_monado vs monado-service (partial rebuild). Workaround `IPC_IGNORE_VERSION=1` (set in run_vr_session.sh); better: `ninja` a full rebuild.
- `GetStringTrackedDeviceProperty(ModelNumber)` returned garbage in the test (minor; check xrizer log with `RUST_LOG=debug` if a game cares).
- Compositor submit/WaitGetPoses (graphics) not exercised; needs a real game or hellovr_vulkan (`~/src/openvr/samples`, needs SDL2/GLEW; not built).

## Running a game
1. Install Steam (`sudo pacman -S steam`, needs multilib enabled) and the game; Proton (Experimental or GE) for Windows games.
2. In a terminal: `analysis/tools/run_vr_session.sh` (monado-service, Wayland direct mode DP-1, mode 2; Ctrl-C to stop).
3. Steam launch options (Proton / pressure-vessel container needs the Monado IPC socket and the runtime json visible):
   `PRESSURE_VESSEL_FILESYSTEMS_RW=$XDG_RUNTIME_DIR/monado_comp_ipc XR_RUNTIME_JSON=$HOME/src/monado/build/openxr_monado-dev.json %command%`
   If the game ignores openvrpaths.vrpath add `VR_OVERRIDE=$HOME/src/xrizer/target/release`.
   Native Linux games usually need only `%command%` (plus VR_OVERRIDE if needed).
   Caveat: the container can't load Monado from a path outside $HOME/$XDG_RUNTIME_DIR; ours is under $HOME so it is visible. The OpenXR loader inside
   the container must be able to load libopenxr_monado.so (built against host libs; if it fails there, try Steam's "Steam Linux Runtime"-less
   launch, or install monado system-wide / build with the same runtime).
   Do not start SteamVR itself. Logs: `~/.local/state/xrizer/xrizer.txt`, `/tmp/omarchyvr-session/monado.log`.
   Game windows: the game opens a flat mirror window; the headset shows the Monado compositor output.
4. Tracking is 3DoF only (no positional tracking) and controllers are being worked on separately, so use seated, head-look games.

## Suggested first tests (seated, no controllers needed)
1. `hellovr_vulkan` / `ovr_smoke` (already done) as a baseline.
2. Free / native-friendly OpenVR apps: "Google Earth VR" (gamepad-free, head look), "theBlu" (seated viewer), "Vivecraft"-less alternatives:
   "Elite Dangerous" or "Euro Truck Simulator 2" / "American Truck Simulator" (native Linux, OpenVR option, seated, keyboard/gamepad), "Project Cars 2"/"Assetto Corsa" (Proton, seated sims; Assetto Corsa via Content Manager VR).
3. "Half-Life: Alyx" and Beat Saber: not recommended yet (need 6DoF controllers).

## Fix: games inside Steam's runtime container could not load Monado (2026-10-13)
Proton/pressure-vessel log: `OpenXR-Loader ... skipping manifest openxr_monado-dev.json ... "libcjson.so.1: cannot open shared
object file"`. The Steam Linux Runtime (sniper) has no libcjson. Fix: build Monado with the bundled cJSON:
`cmake -S ~/src/monado -B ~/src/monado/build -DXRT_HAVE_SYSTEM_CJSON=OFF && ninja -C ~/src/monado/build`.
Client lib deps are then only Vulkan/GL/X11/libc/libstdc++ (all in the runtime). Prefer the game's OpenXR mode if offered
(game -> Monado directly); OpenVR mode goes game -> xrizer -> Monado. Never start SteamVR itself; installing/starting it
rewrites ~/.config/openvr/openvrpaths.vrpath (re-run vr_runtime_switch.sh xrizer; chmod 444 the file).
