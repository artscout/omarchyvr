# SteamVR with the Pimax Dream Air

Goal (user, 2026-10-15): SteamVR is the main way to run all games, with no per-game setup. This document covers:

- the zero-setup Steam fix for the Monado path,
- the SteamVR path (Monado's SteamVR driver inside SteamVR),
- what works and what does not,
- the in-headset test plan.

Status: **built, not yet tried in the headset.** The default stays "Games run with: Monado" until the test below passes.

## 1. Steam for VR games (Monado path, no launch options)

Since pressure-vessel 0.20260925, Steam's game container shares Monado's socket only when
`PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1` is set (LESSONS row 27). Games inherit Steam's environment, so setting the
variable once for Steam covers every game.

- **App:** Ready shows the check "Steam set up for VR games". Press Fix. The same fix also appears in the first-setup Fixes step.
- **Engine:** `pimax vr steam-setup on|off|status`. It writes:
  - `~/.local/share/applications/steam.desktop`: a copy of Steam's own desktop entry. Each `Exec=` line starts with
    `env PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1`. Omarchy starts Steam from this entry (`gtk-launch`). Used from the next Steam start.
  - `~/.config/environment.d/90-pimax-steam-vr.conf`: read by the systemd user manager at login, so after the next
    log-in every way of starting Steam has the variable (`steam://` links, game shortcuts).
  - `systemctl --user set-environment` as a best-effort extra.
- **Once:** quit Steam (Steam menu > Exit) and start it again, or log out and back in. The check then turns green.
  It reads the running Steam's environment.
- **Undo:** `off`, or Undo under Applied fixes. It removes only files that carry the marker `X-Pimax-Steam-VR=1`. A
  `steam.desktop` that is not ours is left alone.
- **Fallback** for a Steam started some other way: the per-game launch option from `pimax vr steam-options`.

## 2. Games run with: Monado or SteamVR

"Games" at the top of the app window is the switch. It is saved in `~/.config/pimax/games.toml` (`runtime = "monado"|"steamvr"`),
which the app and the CLI share. Engine: `pimax vr games monado|steamvr|status`.

| | Monado (default) | SteamVR (experimental) |
|---|---|---|
| `openvrpaths.vrpath` | xrizer only, read-only (0444) | SteamVR first, writable (0644; SteamVR 2.17 exits on a read-only file, LESSONS row 28) |
| SteamVR driver | unregistered | `<monado build>/steamvr-monado` in `external_drivers` |
| OpenXR runtime | Monado (set by `vr start`) | never changed by the switch |
| Play | monado-service + xrizer, as before | SteamVR's vrserver/vrcompositor with `driver_monado.so` |
| Stop Play | `pimax vr stop` | `pimax vr stop`: SIGTERM to vrmonitor by pid, then vrserver/vrcompositor, SIGKILL after 10 s |

With SteamVR chosen, `pimax vr start`:

1. Refuses while any monado-service runs. Both would open the headset (DP lease, video prepare, cameras, eye chip).
   It also refuses while a SteamVR it did not start is running.
2. Writes the Pimax settings monado-service would get (FPS, 6DoF/SLAM, broker, anchor, hands, IPD, scale) to
   `~/.config/pimax/steamvr-driver.env`. The driver reads that file in vrserver before it starts the Pimax driver
   (log marker `PIMAX_STEAMVR_ENV`).
3. Hands the OpenVR file to SteamVR and registers the driver.
4. With 6DoF: starts the camera broker (and ctrltrack once SteamVR is up), as on the Monado path.
5. Starts `SteamVR/bin/vrstartup.sh` directly with `VALVE_SKIP_RUNTIME_SAFETY=1`, so vrserver runs on the host.
   Inside Steam's "sniper" container, `driver_monado.so` cannot load: the one Monado build needs host libraries
   (OpenCV 5, GStreamer, hidapi, Basalt) that the container lacks. Set `PIMAX_STEAMVR_LAUNCH=steam` to start it through
   Steam (`steam://rungameid/250820`) instead.
6. Waits for vrserver (90 s) and records its pid in `<session dir>/steamvr.pid`. The app shows the session as running, and Play becomes Stop Play.

The `--runtime monado|steamvr` start option overrides the saved choice once.

**Flip the default** after a good headset test: `pub const DEFAULT: Games = Games::Monado;` in
`rust/pimax-tools/src/vr/steamvr.rs`. Change it to `Games::SteamVr`. The app and the CLI both read it.

### The driver (Monado, one build)

`XRT_FEATURE_STEAMVR_PLUGIN=ON` is already in the cache of the one build. Every build has
`steamvr-monado/{driver.vrdrivermanifest,bin/linux64/driver_monado.so,resources}` next to monado-service. The driver
links the same Pimax driver, so it carries every marker (`PIMAX_SLAM`, `PIMAX_ANCHOR`, `PIMAX_EYE`,
`PIMAX_CTRL_TRACK`, `held from the tracker`).

The Monado commit "steamvr_drv: Pimax under SteamVR" adds:

- the settings file above;
- EDID props `0xD242/0x1019` ("PVR") so vrcompositor can find the panel for direct mode;
- a single display rate (PIMAX_FPS; at another rate the firmware never lights the panel);
- per-eye render size (it was both eyes, about 4x too many pixels);
- IPD from `OXR_DEBUG_IPD_MM` (it was a fixed 63 mm);
- a null-safe cleanup.

Build: `~/src/monado-eye/build-all-next2`, configured with every cache option copied from `build-all`.

### First headset test (2026-10-15 17:19): fixed

- **vrmonitor aborted at start.** Omarchy sets `QT_QPA_PLATFORM=wayland;xcb`, and SteamVR's bundled Qt has only xcb
  ("could not find or load the Qt platform plugin"; minidumps `crash_*_2.dmp` = vrmonitor, SIGABRT). SteamVR then
  ended the session. vrcompositor had already taken the lease ("Acquired drm display!", "Direct mode: enabled").
  Fix: the launch sets `QT_QPA_PLATFORM=xcb`.
- **90 vs 72 Hz.** The driver woke the panel at PIMAX_FPS=72 (`pimax_video_prepare(72) -> 0`), but vrcompositor
  chose mode 3, 7104x3840@90, from `steamvr.preferredRefreshRate` (default 90). It ignores the driver's rate list.
  At another rate the firmware never lights the panel. Fix: before each start, `vr start` writes
  `preferredRefreshRate` = PIMAX_FPS into `~/.local/share/Steam/config/steamvr.vrsettings`, keeping the other keys.
- **vrserver log spam (9 MB).** The Touch profile template registers both hands' inputs, so vrserver logged
  "Input ... not found" at 1 kHz. An uninitialised skeleton handle also caused "UpdateSkeletonComponent failed. Invalid
  handle" plus the driver's "Unsupported input: XRT_INPUT_HT_UNOBSTRUCTED_*". Fix (Monado, driver): register only the
  inputs the device has, and zero the skeleton handle.
- **SteamVR started from Steam's own button** runs in the container. There the driver cannot load
  (`libgstapp-1.0.so.0: cannot open shared object file`, vrserver.txt). Use the app's Play.

### Second test (17:27): compositor killed right after "Startup Complete", fixed

vrserver's scheduler setup sets `RLIMIT_RTTIME` to 0 (`/proc/<vrserver>/limits`: Max realtime timeout 0). Inside
Steam's container the compositor is started by Steam's launcher service; on the host it is vrserver's child and
inherits the 0. vrcompositor-launcher's `cap_sys_nice` then gives it realtime threads, and the kernel SIGKILLs it
(no log line, no dump). Measured with the same launcher: rttime 0 is killed after 1 s, unlimited keeps running.

Fix: the Monado build has `steamvr-monado/bin/linux64/pimax_rttime_preload.so` (marker `PIMAX_RTTIME_PRELOAD`). It
makes setrlimit/prlimit on RLIMIT_RTTIME a no-op, and `vr start` puts it in `LD_PRELOAD` for SteamVR. Verified live:
the compositor kept running with SteamVR Home connected, the limit was unlimited, and there was no Xid.

## 3. What works and what does not (code reading, not yet tested)

Expected to work:

- **Panel wake and keepalive rate.** `pimax_video_prepare` and `pimax-rate` are in the driver, not in the compositor.
- **Distortion.** It comes from the pimax-lens LUT through `compute_distortion`. The FOV is from lens.conf.
- **Canted views.** They come from the view poses through `SetDisplayEyeToHead`.
- **3DoF IMU tracking.** It is in-process.
- **6DoF SLAM, the IMU gate, the anchor, Mercury hands (the hands model itself) and the ctrltrack reader.** They run
  in-process once the settings file is read. The broker and ctrltrack are separate processes that `vr start` starts.
- **Pimax controllers.** They show up as Oculus Touch with every button, stick, touch and the finger skeleton (section 5).

Will not work, or is different:

- **Monado compositor safety.** There is no flipsafe and no bounded GPU wait: a stuck NVIDIA flip hangs vrcompositor
  instead of releasing the lease. After an Xid error, reboot, as always.
- **Realtime priority.** There is no realtime thread priority unless SteamVR's own vrcompositor-launcher has it.
- **Monado-service-only features.** The recorder tap (`pimax-record`), the wrist perf readout (`PIMAX_PERF_FILE`) and
  WayVR (Work) belong to monado-service. Work always uses Monado.
- **Eye gaze.** The SteamVR driver has no eye-tracking component.
- **Camera hands as controllers** (`PIMAX_HT_ROLES=hands`). The grip pose falls back to the head.
- **Controller vibration.** SteamVR sends haptic pulses to the driver, but the Pimax haptics command is unknown (no
  Windows capture has one), so nothing vibrates.
- **Native OpenXR games** keep using Monado's OpenXR runtime (the switch never changes it). With SteamVR chosen they
  need SteamVR set as the OpenXR runtime (SteamVR's settings), or Monado.

Unknown until tried:

- whether vrcompositor takes the Wayland DRM lease of DP-1 on NVIDIA under Hyprland;
- whether each eye's image is turned 180 degrees (ovrd applies the view rotation to UV, Monado's compositor to the mesh);
- whether SteamVR started outside Steam (`VALVE_SKIP_RUNTIME_SAFETY=1`) opens its dashboard. Steam must be running for that.

Risks:

- a black panel at a rate mismatch;
- a hung vrcompositor (no flipsafe);
- a 180-degree image;
- SteamVR's own crash at exit.

None of these needs unplugging the headset. Stop Play closes SteamVR by pid; Restart headset works as usual.

## 4. In-headset test (app buttons only)

Do this when nothing else runs (no Play, no Work).

1. "Games" (top of the window) > **SteamVR (experimental)**. The message line says "Games run with: SteamVR".
2. Press **Play**. The status line says "SteamVR is up". Put the headset on within a minute.
3. Check, and say out loud or note:
   - Is the panel lit?
   - Is the image upright, sharp in the middle, with straight lines (lens test pattern if needed)?
   - Does turning your head look right?
   - With 6DoF on: does leaning move you?
   - Do both controllers show and react?
4. In SteamVR's home, open the dashboard (menu button). Then start Dungeons of Eternity from Steam.
5. Press **Stop Play** in the app. SteamVR closes.
6. If anything failed: "Games" (top of the window) > **Monado** puts everything back. Help > Logs has
   `~/.local/share/Steam/logs/vrserver.txt`, `vrcompositor.txt` and `/tmp/pimax-vr-session/steamvr-launch.log`.
7. If all of it worked: tell the main thread to flip `DEFAULT` (section 2).

## 5. Controllers in SteamVR (Monado commit "steamvr_drv: Pimax controllers as Oculus Touch", 2026-10-15)

**Choice: present them as Oculus Touch, exactly.** The controllers use SteamVR's own Touch input profile,
`{oculus}/input/touch_profile.json`, which ships with SteamVR in `drivers/oculus` (only its `.so` is missing on Linux).
They also use the Quest 2 render model. Games ship default bindings for `oculus_touch`, SteamVR has a legacy (OpenVR 1)
binding and a dashboard binding for it, and the binding UI shows Touch pictures. A custom `pimax_controller` type would
need its own bindings in every game, so Touch gives the best compatibility.

Before, the controllers used Monado's generated Touch profile. Its trigger was a "button" with no value, the left
menu button sat on `/input/menu` (so the dashboard could not open), the grip had no touch, there was no skeleton,
and the pose was the OpenXR grip, not the Touch "raw" pose.

| Physical (Pimax) | SteamVR path | What games see |
|---|---|---|
| Trigger | `/input/trigger/value`, `/input/trigger/touch` | Touch trigger (pull 0..1, touch above 6 %; click comes from the pull) |
| Grip | `/input/grip/value`, `/input/grip/touch` (squeeze > 5 %) | Touch grip |
| Stick | `/input/joystick/x`, `/y`, `/click`, `/touch` | Touch thumbstick |
| X / Y (left) | `/input/x/click`, `/input/x/touch`, `/input/y/click`, `/input/y/touch` | X, Y (legacy: A and app menu) |
| A / B (right) | `/input/a/click`, `/input/a/touch`, `/input/b/click`, `/input/b/touch` | A, B |
| Menu button (either hand) | `/input/system/click` | SteamVR system button: opens the dashboard |
| Fingers (curl from trigger, grip, touches; pimax-hands) | `/input/skeleton/left`, `/input/skeleton/right` (Partial) | Hand skeleton (SteamVR Home hands, Skeletal Input games) |
| Vibration | `/output/haptic` | Vibrates (section 6; Monado e865c1a, build-all-next8) |
| Pose | device pose = Touch raw = grip x inverse(Quest 2 `openxr_grip`) | `/pose/raw`, `tip`, `base`, `handgrip`, `openxr_grip`, `openxr_aim` from the model |
| Battery | `Prop_DeviceBatteryPercentage`, `Prop_DeviceIsCharging` (every second) | Battery in the dashboard |

The Touch thumbrest (`/input/thumbrest/touch`) has no Pimax counterpart and is not registered. SteamVR does not
complain about inputs that are missing.

**Head position (same commit).** With 6DoF on (`PIMAX_SLAM=1` in the settings file), the HMD pose now has
`shouldApplyHeadModel = false` and `willDriftInYaw = false`. Before, SteamVR added its neck model on top of the tracked
position, so every head turn moved the view twice ("the head moves a bit too much"). The choice is made once per
session, so a short tracker dropout does not make the view jump. A held position (tracker not trusted) is now kept, not
reset to the origin, because the driver copies a VALID position instead of only a TRACKED one. vrserver.txt says
`monado: HMD Pimax Dream Air: 6DoF (position tracked), SteamVR head model OFF`. Units are metres in both Monado
and SteamVR, and the 1.6 m floor offset is added once.

**Live check (17:53, build-all-next5-input).** vrserver.txt lists every path above for both hands, plus
`Added skeleton input /skeleton/hand/left|right`, with no "not found" or "not registered" lines. The legacy binding
(`legacy_bindings_touch.json`), the compositor binding and SteamVR Home's `bindings_touch.json` loaded for
`oculus_touch`. "Controller (/user/hand/left/input/system) not available for input to chord" comes from the generic
HMD's compositor binding, which loads before the Touch one, and is harmless.

## 6. Controller vibration (haptics, 2026-10-15)

**Wire format** (Windows capture 2026-10-15, takes 14 and 15, checked byte for byte with `analysis/tools/usbp.py`):
one 64-byte report on the headset's command endpoint (EP 0x01, hidraw report id 0), no reply:

```
58 09 07 SS AMP DUR_lo DUR_hi 00 00 00 (zero padded to 64)
SS = 02 left, 03 right; AMP = 1..100 %; DUR = u16 little endian, ms
```

Pimax Play's test pulse is `58 09 07 03 19 2c 01` (right, 25 %, 300 ms). Dungeons of Eternity streamed a new command every
8-10 ms per hand with DUR 11-12 ms and a changing AMP (an envelope). AMP 0 was never sent: a burst just stops.

**How it is wired (one build, always on).**
- libpimax (`rust/pimax`, `pimax-proto::haptic_command`): `pimax_haptic(h, slot, amplitude 0..1, duration ns)` maps to
  AMP = round(amplitude x 100) clamped 1..100, DUR = ceil(ns / 1 ms) clamped 1..65535, `XR_MIN_HAPTIC_DURATION` (and 0)
  -> 12 ms like games. Frequency is ignored (not in the protocol). Amplitude 0 = stop: nothing is sent, a waiting pulse
  is dropped. The command goes out on the session thread (the one that owns the HID), at most once per 8 ms per hand;
  a newer request replaces a waiting one (newest wins).
- Monado Pimax driver (`pimax_ctrl.c`, marker `PIMAX_HAPTICS`): `set_output` of `/output/haptic` calls it. OpenXR games
  (`xrApplyHapticFeedback`) and SteamVR games (through driver_monado, `/output/haptic`) both reach it. driver_monado now
  gives the vibration value its type (it was uninitialised) and logs only the first haptic event.
- Test pulse: `pimax controllers buzz left|right [--amp 25] [--ms 300]` and the app's **Test vibration** button on each
  controller card. While Monado or SteamVR runs, the request goes through the session: `SLOT AMP MS` in
  `$XDG_RUNTIME_DIR/pimax-haptic-request`, taken by the driver within 100 ms (log `PIMAX_HAPTICS: test pulse on slot N`).
  If nobody takes it within 1 s the request is removed and the app says the build has no vibration support. With no
  session the command is written straight to IF0. The controller must be awake and paired.

**To test in the headset:** Home > Controllers > Vibrate (each hand), then a game with hits (Dungeons of
Eternity). monado.log / vrserver.txt show `PIMAX_HAPTICS: first vibration (...)` once.

## Live-test fixes (2026-10-15, SteamVR mode)

- **OpenXR runtime follows the choice.** `pimax vr start` / `pimax vr games steamvr` point `~/.config/openxr/1/active_runtime.json` at SteamVR's `steamxr_linux64.json` (no monado-service runs, so native OpenXR games and Proton `-openxr` would otherwise find nothing); `pimax vr games monado` points it back at the build's `openxr_monado-dev.json`; a Monado start (Work / `desk start`) points it at its build only when Games run with: Monado (with SteamVR chosen it leaves the link alone: WayVR and the session's apps get `XR_RUNTIME_JSON` directly; games are played from Play; docs/GAMES-CHECK.md "Work mode"). A regular file is saved once as `active_runtime.json.pimax-backup`. `pimax vr steam-options` prints `PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1 %command%` in both modes (the container imports the host's OpenXR runtime either way).
- **`pimax vr game-check` (and the app's Games check) follows the choice.** SteamVR mode: vrserver + vrcompositor running, OpenVR = SteamVR first with driver_monado in `external_drivers`, OpenXR = SteamVR's own, container test only once SteamVR runs.
- **Display frequency.** vrcompositor overwrites `Prop_DisplayFrequency_Float` with 90 right after start although it selected the 72 Hz mode; driver_monado (Monado 38a0523) sets it back to PIMAX_FPS from RunFrame (log: "Display frequency was changed to ... set back"). The "recommended 3840x3552 90.0Hz" line is vrcompositor's own, read before the driver can correct it.
- **Backlight.** In SteamVR mode vrserver owns the HID: at 100 % (the default, already what the panel has) nothing is sent and nothing is said; below 100 % a failure is explained in plain words.

## 7. Hermetic launch: SteamVR's settings stay in SteamVR (incident 2026-10-16)

**What happened (10:51-10:53).** Steam was not running when Play started SteamVR (this boot, Steam's
`connection_log.txt` shows SteamVR's in-process steamclient at 10:49 "no bootstrapper found"). In the SteamVR
dashboard the user picked Half-Life 2: VR Mod (`vrserver.txt` 10:51:06 `OnMsg_LaunchApplication( steam.app.658920 )`).
vrserver opens such links with a shell: `STEAMVR_VRENV= STEAM_RUNTIME= LD_LIBRARY_PATH="$SYSTEM_LD_LIBRARY_PATH"
QT_PLUGIN_PATH= xdg-open 'steam://launch/658920/VR'` (string in `bin/linux64/vrserver` and `vrmonitor`). With no
Steam to hand the link to, `xdg-open` (pid 15733, parent systemd --user after its parent exited) started
`steam.sh` and the Steam client (pid 16972) **as a child of SteamVR, with SteamVR's whole environment**:
`SteamAppId=250820`, `SteamGameId=250820`, `STEAMVR_TOOLSDIR`, `STEAMVR_VRENV` (exported by SteamVR's
`vrenv.sh`), and ours: `LD_PRELOAD=…pimax_rttime_preload.so`, `QT_QPA_PLATFORM=xcb`, `VALVE_SKIP_RUNTIME_SAFETY=1`,
`PIMAX_*`, `OXR_DEBUG_IPD_MM` (`/proc/16972/environ`). With `SteamGameId=250820` the Steam client took itself
for SteamVR: 6 asserts in its first minute (`/tmp/dumps/assert_20261016105153_27` … `_105257_53`; earlier Steam
starts had one at most), among them `CClientGame::Init: passed own procID as game procID` and "Adding process 16972
for gameID 250820". Steam connected to vrserver at 10:52:35, SteamVR's shutdown was requested 3 s later (vrmonitor
10:52:38, "quit all" 10:52:45), and Steam's relaunch of 250820 recorded its own pid, so SteamVR never came back; the
game then started (10:53:43) without VR and quit. Not the cause: `LD_PRELOAD` (the 32-bit Steam client cannot load the
64-bit preload, "wrong ELF class: ignored"; in 64-bit helpers it only turns RLIMIT_RTTIME calls into no-ops).

**Fix (`rust/pimax-tools/src/vr/steamvr.rs`).**
- **Steam first.** Before SteamVR, Play checks for a Steam client (`~/.steam/steam.pid`, comm `steam`). None:
  it starts Steam through its desktop entry, `env -u … gtk-launch steam` (our `steam.desktop`, so Steam keeps
  `PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1`), and waits up to 60 s for Steam and its `steamwebhelper`. A steam://
  link from SteamVR then only goes to the running Steam. Steam's pid is kept in `<session>/steam-client.pid`.
- **A Steam that carries SteamVR's variables: Play asks** (2026-10-16 18:22: the Steam of 10:51 was still open, Play
  failed with "Steam (pid 16972) was started from inside SteamVR ..."; it predated these shims, installed 18:06).
  The app (facts `steam_tainted`, read from `/proc/PID/environ`) opens its own dialog "Steam must restart for VR.
  Restart Steam now?" [Restart Steam] [Cancel], with a warning when a game (`reaper` process) or a download
  (`steamapps/downloading`) seems to run. Only on yes, one job runs `pimax vr steam-restart`: `steam -shutdown`
  through `env -u ...` (clean environment), waits up to 90 s for that pid to exit, then Play goes on and starts Steam
  the normal way. If Steam does not exit, the job stops with "did not close within ... Nothing was forced": never a
  signal to Steam. A clean or absent Steam is never closed. Cancel starts nothing. The CLI `pimax vr start` still
  refuses in plain words; start at login does not start and says Play must be pressed once. No dialog in the
  headset: at that moment no session runs, so there is nothing in the headset to show it on.
  Tests: `restart_*`, `a_tainted_steam_is_found...`, `play_asks_before_closing_a_tainted_steam...`.
- **Shims in SteamVR's PATH.** `<session>/steamvr-shim/xdg-open`, `steam`, `gio` and `gtk-launch` go first in SteamVR's `PATH`; they run
  the real program through `env` without SteamVR's and our variables (`STEAMVR_ONLY` + the Pimax ones) and with the
  user's own `PATH`, `QT_QPA_PLATFORM`, `LD_LIBRARY_PATH`, `LD_PRELOAD`, `QT_PLUGIN_PATH` back. So a web page or a
  Steam started from SteamVR gets the normal environment even if Steam was closed meanwhile.
- **Which variable goes where** (test `env_builder_keeps_steamvr_variables_in_steamvr`): vrstartup.sh (and so
  vrmonitor, vrserver, vrcompositor) gets `VALVE_SKIP_RUNTIME_SAFETY`, `QT_QPA_PLATFORM=xcb`, `LD_PRELOAD` (rttime
  preload), the Pimax driver variables and the shim `PATH`; Steam and everything opened from SteamVR get none of them.
  `PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES` is never removed.
- **During the session.** Home (and `pimax vr status`, line `steamvr: …`) says in plain words when Steam restarted or
  closed, when the running Steam came from inside SteamVR, or when SteamVR quit on its own. `pimax vr stop` with a
  vrserver that is already gone closes the leftover vrmonitor/vrcompositor by pid and removes the pid files and shims.
- Not covered: SteamVR's own "Restart SteamVR" goes through Steam (`steam://run/250820`), which starts SteamVR in
  its container without the Pimax driver's libraries; use Stop Play / Play instead.

