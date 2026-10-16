# Pimax app (GUI)

The desktop app to install, run and diagnose the Pimax Dream Air stack on Linux. Window title "Pimax", binary
`pimax-setup`. It does not replace the tools: it calls `pimax` (the launcher), `pimax-doctor` (as a library), `pimax-eye`,
`pimax-cam`, `pimax-roommap`, `pimax-anchor`, `wayvrctl`, the auto mode daemon and the analysis scripts, and shows what
they do. Everything it may run is checked against the rules of [LESSONS.md](LESSONS.md) and the safety rules of
[INVENTORY.md](INVENTORY.md) section 6 before it runs.

![Home](gui/desk-idle.png)

## Run

```bash
cd ~/pimax-re/rust && cargo run --release -p pimax-setup      # or, once installed: pimax-setup / "Pimax" in the launcher
pimax-setup --demo               # the window with built-in demo data (mock headsets); nothing is started or sent
pimax-setup --renderer wgpu      # Vulkan, only in builds with `--features wgpu` (glow = OpenGL is the default; also PIMAX_RENDERER)
pimax-setup --no-supervisor      # the window in this process (no restart after a crash)
pimax-setup --self-test          # read-only: collect everything once, print a summary and the items that need attention
pimax-setup --report             # write a redacted problem report (.tar.gz) and print its path
pimax-setup --screenshots DIR    # render every page from demo data to DIR/*.png (CPU only: no window, no GPU)
pimax-setup --screenshots DIR --real   # the same from this machine's state (read-only), for bug reports
pimax-setup --version
```

Install (no root): Setup > Launcher and app > This app > Fix builds the release binary, installs it to
`~/.local/bin/pimax-setup`, the icon `linux/pimax.svg` and the desktop entry `linux/pimax.desktop` ("Pimax") to
`~/.local/share/applications`. Or as a package: [packaging/README.md](../packaging/README.md) (Arch `PKGBUILD`, an
AppImage script). Environment: `PIMAX_REPO` (default `~/pimax-re`), `PIMAX_VR_SESSION_DIR`, `PIMAX_MONADO_BUILD`,
`PIMAX_RENDERER`, `PIMAX_NO_SUPERVISOR=1`, and the doctor's `PIMAX_SYSFS_ROOT` / `PIMAX_PROC_ROOT` / `PIMAX_DEV_ROOT`.

Desk mode (and the VR session of a tool) is started in its own transient user scope
(`systemd-run --user --scope --unit pimax-desk-<time> -- pimax desk start ...`, when the user manager is there), so
Monado never lives in the app's cgroup: closing the app, a crash of the app, or the desktop launcher cleaning up the
app's scope does not stop it. Stop it with the Stop button (or `pimax desk stop`). A start's output goes to a file
(`~/.local/state/pimax/jobs/`) that the app follows, so a start goes on if the window dies in the middle of it.

The first start opens the onboarding (until it is finished or closed); later starts open the last page. The app
remembers whether the onboarding was finished, the switches, the supersampling and the last page in
`~/.config/pimax/gui-state.toml`; the onboarding's own progress is in `~/.local/state/pimax/onboarding.state`.

## Look

After Pimax Play (reference: the Home screen of the Windows app): near-black rounded cards, an indigo accent, outline
icons (Phosphor, only the glyphs used are compiled in), Noto Sans when the system has it; a light palette follows the
desktop. Rows expand in place with an eased height, tabs, hover and status pills fade instead of jumping, a new page
fades in. The window repaints only on a change or while something animates (about one frame per second when idle),
without vsync and at most 60 frames per second.

| Before | After |
|---|---|
| ![before](gui/before/desk-idle.png) | ![after](gui/desk-idle.png) |

## Pages

The top bar has the pages (Home, Setup, Tools, Logs, Profiles, AI, System; Onboarding until it is done) and Re-check; the bottom
bar the device dots (headset, left and right controller, tracking mode), the running action or the last message, the
output panel, Setup and Help. A round help button opens the controls guide, this document, the lessons and "Report a
problem".

**Home.** Left: settings rows, each with its current values on the right; a click opens it in place.

| Row | What |
|---|---|
| Quality / Performance | Refresh rate 72/90 Hz; supersampling 0.5-2.0x (`XRT_COMPOSITOR_SCALE_PERCENTAGE`, passed to Monado by the launcher); eye tracking; foveated rendering (coming soon); the performance readout on the right wrist (`--no-perf` when off). |
| Optics / Display | IPD 58-72 mm with Apply (moves the lenses with the captured motor command when no session runs, shows "Lenses now" from the headset, and writes `~/.config/pimax/ipd.env`, used at the next start; while a session runs the lenses do not move and the page says why), "Measure with the eye tracker" (the IPD tool); brightness (coming soon); camera auto exposure state; lens test patterns. |
| Tracking | 6DoF (`--6dof`), optical controller tracking (`--ctrl-track`, experimental), the 6DoF and gyro calibration state now; walk test, controller recording, hands test. |
| Eye tracking | "Eye tracking on at desk start" (on by default; with a launcher that knows `--no-eye` the app passes `--no-eye` when it is off, else `--eye` when it is on), "Eye laser" in the running session (`wayvrctl gaze-laser on|off`), the calibration file and its validated accuracy, Calibrate / Eye view / Power on eye module, the refit. |
| Desktop / WayVR | Subtitles (below), passthrough, the desk preset (`--preset cinema\|work\|assistant`), WayVR live actions, the settings of the next start, the conf.d check, the saved layout, the controls guide (below). |
| Room | The anchor hook (`--anchor`), Map my room / Recognise room, the AprilTag (below). |
| Voice | Live captions (`--voice`), the screen they sit under, the voice test, the voice commands research page. |
| Video recording | Coming soon (greyed). |
| Auto mode | The auto mode daemon (below). |
| VR mode / Runtime | xrizer + Monado or SteamVR (`vr_runtime_switch.sh`), the Steam launch options, the Proton note (LESSONS row 23). |
| Developer | Logs, Setup checklist, crash reports, Report a problem, the app version and log file, what Start runs. |

Right: the runtime selector, the tracking badge (INSIDE-OUT 6DoF, 6DoF LOST, 3DoF), the headset card with a model
picker (the headset modules of `pimax-device`: the Dream Air is real; Dream Air SE, Crystal Light, Crystal Super and
8KX are mocks whose capabilities read "not supported yet"; the model is matched on the USB list, an unverified id of
a mock shows as "possibly ..."), the controller cards (connected / tracked from monado.log; battery unknown, marked
mock in demo data; Pair, at most once per hand every 30 s), the headset menu with **Restart headset** (below) and a
"Camera missing" state when the headset is on USB but its camera is not, **Start Desk** (`pimax desk start ...`), **Start VR** (`pimax vr start --fps N [--6dof [--ctrl-track]] [--perf]
[--anchor]`, Monado alone for Steam and OpenXR games; the app never launches a game) and **Stop**, the preflight verdict
with "Start anyway" when only WayVR Xid 31 faults stand in the way and the full preflight below it, a small Subtitles
switch (live while a desk session runs, else the default of the next start), ROOM SETUP (opens the Room row), and the
session details.

**Subtitles** (live captions of what you say, under the main screen; desk mode only, VR mode has no WayVR panel):
"Subtitles now" acts on the running session through `pimax desk captions on|off` (the same switch as the voice
commands "captions on/off": the flag file `$XDG_RUNTIME_DIR/pimax-voice-captions.off`; `on` also starts `pimax-voice
listen` and `pimax-voice captions` when they are missing). A session started without voice has no captions panel in
WayVR: the launcher says so, and the next start needs the default on. "Subtitles on when desk starts" is the default,
saved with the app's settings: Start Desk adds `--voice`. Both are in the Desktop / WayVR row; demo mode only
simulates. With an older launcher (no `desk captions`) the app flips the flag file itself.

**Restart service** (headset menu on Home, next to Restart headset): `pimax desk restart` or `pimax vr restart`, whichever
started the session (`start-argv.txt` in the session dir): stop with 30 s grace, then start again with the same
switches, as a job with a progress bar. Under the headset card: "Auto-reconnect on" and the watchdog's last log line
while the session watchdog runs (`watchdog.pid`, `watchdog.log`; the launcher restarts a session whose Monado,
headset or camera went away, at most 3 times in 10 minutes). Both need a launcher that has `restart` (from
`pimax desk help`); an older one: the button says to install the current launcher.

**Restart headset** (headset menu on Home; Setup > Checks when the camera is missing): `pimax vr reset-headset` as a
job. It sends the headset its own restart command `58 04 28 00 00` (what Pimax Play sends), watches the USB bus (HID
back after about 8 s, camera after about 17 s, picture after about 22 s) and shows each stage in plain words on a
progress bar of about 30 s. If the headset ignores the command (nothing leaves USB within 15 s) the old slow restart
(heartbeat timeout, up to 2 minutes 30) follows and the bar says so. Refused while Monado runs. The launcher's verdict line becomes one sentence ("the camera is back after 74 s", or "try Restart
headset once more"); the app never tells anyone to unplug anything. "Camera missing" comes from the USB list (HID chip
34a4:0044 present, camera 34a4:0014 not), read in the background with the other facts.

**Setup** has three tabs. **Checks**: the system checks of `pimax-device` (its own and every pimax-doctor check through
`DoctorBridge`) grouped by area, each finding with its catalogue fix, and the applied fixes with Undo (see "Checks and
fixes" below). **Install**: the checklist as before (pimax-doctor plus the LESSONS checks, builds, launcher, this app;
user fixes run directly, root fixes show the documented `sudo` lines, Copy and "Run with pkexec...") plus the manual
steps of [MANUAL-STEPS.md](MANUAL-STEPS.md) it covers: cargo, Steam and Proton (pacman lines copy only), the keepalive
unit with its Rust drop-in, the auto mode daemon (build, user unit, dry-run `auto.toml`), the installed EDID and
Hyprland's DP-1 line (read only: the app never edits Hyprland's config). **Autofix**: ask a language model (below).
Home also has the eye calibration refit (Eye tracking: Refit the newest run, Save the refit, confirmed; Save waits
until the eye build reads version 2 files), the controller gyro fit (Tracking, from the last session's logs) and
Start / Start at login for the auto mode unit. **Tools**: each tool with what keeps it from starting and a button that resolves it.
**Logs**: the session logs (and the app's own log), history, open issues, lessons, safety rules. **Profiles**:
per-headset profiles and backups. **AI**: the language model providers (below). **System**: pimaxd's view of the stack (below). **Onboarding** (replaces
the first-run wizard): below.

Screenshots (demo data, `pimax-setup --screenshots docs/gui`):

| | |
|---|---|
| ![Home](gui/desk-idle.png) | ![Home, running](gui/desk-running.png) |
| ![Quality and Desktop rows](gui/home-rows.png) | ![Room and Voice rows, a mock headset](gui/home-room.png) |
| ![Auto mode row](gui/home-auto.png) | ![Home, light](gui/desk-idle-light.png) |
| ![Setup](gui/setup.png) | ![pkexec dialog](gui/setup-pkexec.png) |
| ![Tools](gui/tools.png) | ![Logs](gui/logs.png) |
| ![History](gui/history.png) | ![Lessons](gui/lessons.png) |
| ![Setup: Checks](gui/setup-checks.png) | ![Setup: Autofix with the demo answer](gui/autofix.png) |
| ![Onboarding: find the headset](gui/onboarding.png) | ![Onboarding: pair the controllers](gui/onboarding-pair.png) |
| ![AI providers](gui/ai.png) | ![AI: editing a provider](gui/ai-edit.png) |
| ![System (pimaxd)](gui/system.png) | |
| ![Profiles](gui/profiles.png) | |

## System (pimaxd)

The System page shows what pimaxd ([PIMAXD.md](PIMAXD.md)) knows: the snapshot (schema v1) from `{"cmd":"state"}` on
`$XDG_RUNTIME_DIR/pimax-autod.sock`, asked every 2 s in the background while the page is open. Without an answer it
uses the mirror `$XDG_RUNTIME_DIR/pimaxd/state.json` while it is at most 10 s old, then a read-only `pimax-health
--json` probe (every 10 s), then says "pimaxd not running". The source is shown at the top.

- **Components**, one row each in table order (devices, processes, features, settings, jobs): a status dot (ok green,
  starting / degraded / blocked yellow, down / failed red, off grey, unknown blue), the health and why ("waits for
  usb.cam"), the restart budget and breaker, the last exit. The switch sends `toggle` for live components; components
  that need a restart (6DoF, eye, ctrltrack, Monado) say so and apply at the next start. **Fix** (`fix`) on what is down,
  failed, has an open breaker or an unexplained exit.
- **Session**: the intent (on / off, by whom, desk or vr) with its switch (`set session=on|off by=gui`) and what runs.
- **Safe mode** (`safe-mode`), the reason when pimaxd went into it by itself (Fix pimaxd), observe only, the next
  action.
- **GPU banner** when the gate says caution: the kernel lines and **Acknowledge** (`ack-gpu`). **Reboot banner** when
  a reboot is needed (an Xid, or a driver that does not match the kernel module): nothing starts automatically; "Start a
  session anyway, once" sends `set session=on force`.
- **Recent events** from the `{"cmd":"events"}` stream (read by its own thread while the page is open, reconnecting
  every 5 s).

Every action is one request; turning a session or a component off, Fix, leaving safe mode, acknowledging the GPU and
the forced start ask first. The switches wait while only the mirror or the probe answers. Demo mode shows a fixture.

## Onboarding

The steps of the headset's module (`[onboarding] steps` in its `device.toml`): find the headset, check this computer,
fix what the checks found, udev access, IPD, pair the controllers, eye calibration, map the room, done. The mocks
only list find, checks, fixes, udev and done. A stepper shows every step with its state (done, skipped, failed).
Each step has **Run** (its action), **Next** (marks it done; only when its condition holds, e.g. the udev rule is
installed and the headset nodes are writable, an IPD is saved, both controllers paired, a room map exists), **Skip**
and **Back**. The progress is `~/.local/state/pimax/onboarding.state` (`pimax_device::onboarding::Progress`, written
atomically in the background), so the page resumes at the first step that is neither done nor skipped; a step whose
job failed comes back first. **Start over** deletes the file. Help > Onboarding opens it again later.

| Step | Run |
|---|---|
| Find the headset | The USB list matched against the modules (`pimax_device::detect`); pick a model when it is not plugged in. |
| Check this computer | The checks below, again. |
| Fix what the checks found | The failing findings with their fixes (as on Setup > Checks). |
| udev access | The `udev.install_rules` catalogue fix: the pkexec dialog with the lines first. |
| IPD | `pimax-eye ipd --watch` in a terminal (the IPD tool; `a` sets the lens motor once). |
| Pair the controllers | Pair left / Pair right (one StartPair through auto mode per click, at most one per hand every 30 s). |
| Sound in headset / Test headset sound | Home: while VR runs the headset speakers (DisplayPort audio, the NVIDIA HDA card's "Pimax" port) are the default output; Stop restores it. Test headset sound plays a chime there only (docs/AUDIO.md). |
| Headset microphone / Test microphone | Home: the headset microphone is the default input while VR runs; "Microphone: headset / other" shows the input now; Test microphone records 3 s, shows the level, plays it back (docs/AUDIO.md). |
| Test vibration | Each controller card: a 25 %, 300 ms buzz (`pimax controllers buzz`), through the running VR session when there is one (docs/STEAMVR.md section 6). |
| Eye calibration | `pimax-eyecal` in its own session (the eye calibration tool). |
| Map the room | Opens the Room row on Home (the room setup page; its maps complete the step). |

## Checks and fixes

Setup > Checks lists `Vec<Finding>` by area (headset, USB and access, drivers, GPU and display, CPU, audio, runtime),
worst first. The checks run with the doctor in the background (every 60 s and on Check again); a doctor check that
`pimax-device` has ported (setcap, merged_head_sync) is listed once. A finding's fix is an id from the fixed catalogue
(`pimax_device::check::CATALOGUE`) rendered by `FixRef::plan()` (arguments are plain paths or words only):

- a user fix runs as a job (its output in the panel below, the safety rules checked again before each command);
- a root fix opens the pkexec dialog with the exact lines and the undo; nothing runs before you confirm it there
  (lines the safety rules do not allow through pkexec are copy only);
- a reboot is only shown (the app never reboots for you).

Every fix the app ran, or opened in a terminal, is appended to `~/.local/state/pimax/fixes.log`
(`pimax_device::autofix::log_line`: time, id, outcome, commands, undo; the file is private to you). "Applied fixes"
lists the newest 30 with **Undo** (the undo lines, run the same way) or **Apply again** after an undo.

## AI (the AI tab)

The language models the app and voice control use, in `~/.config/pimax/ai.toml` (`pimax_config::ai`, shared with
`pimax-voice`):

- **Providers**: Claude (Anthropic; models `claude-haiku-5-5` (fast, the default), `claude-sonnet-5-5`,
  `claude-opus-5-5`), OpenAI (free-text model), or any OpenAI-compatible server (ollama, llama-server, LM Studio, a
  hosted one) with its base URL. Add, edit (name, kind, model, base URL, key, or an environment variable for the key),
  remove.
- **Keys**: a masked field with Show / Hide. Saved in ai.toml (written atomically, mode 600, readable only by you; a
  load warns when others could read it), or kept in memory for this session only when "Save the key" is unticked
  (then only Autofix, which runs in the app, can use it). Keys never appear in logs, reports or the Autofix bundle.
- **Voice control with AI**: on or off (`voice_ai`; off = the offline command matcher only, nothing you say leaves
  the PC), and when: only for commands the matcher does not understand (`fallback`, the default) or always; "Do not
  send window titles" (`strip_titles`, privacy for window control).
- **Which provider for what**: the default, and one per feature (Voice control, Autofix).
- **Advanced** (folded): the answer timeout (`voice_ai_timeout_s`; default 4 s local, 8 s remote), gaze with a voice
  request (`voice_ai_gaze`), the AI log (`voice_ai_log`); written only when changed from the default. Keys, `[use]`
  entries and sections the app does not know are kept as they are when it saves ai.toml.
- **Local model on this PC** (`pimax-ai`, [AI.md](AI.md)): the status from `pimax-ai status --json`, run in the
  background every 5 s while the tab is open: installed or not, running on the GPU or the CPU, loading or ready, VRAM
  against the 4 GiB budget. Install (4.1 GB, resumes), Start on the GPU (`serve`), Start on the CPU (`serve --cpu`),
  Stop and Test are jobs; Start runs in its own user scope so the server outlives the app. Starting on the GPU while
  a VR session runs is allowed; the page shows the budget next to it. "Use local model for voice" adds the provider
  (openai-compatible, `http://127.0.0.1:8080/v1`, `pimax-local`) and picks it for voice control. Without the binary:
  Build pimax-ai.
- **Last AI actions**: the newest lines of `~/.local/state/pimax/logs/ai.log` (voice control's AI actions), read in the
  background every 5 s while the tab is open.
- **Test connection**: the only network call of the tab, on click: one fixed line ("Reply with the single word OK.")
  to that provider, nothing from this PC; the answer or the error shows under the provider.
- **Local models on this PC**: ollama (127.0.0.1:11434) and llama-server (127.0.0.1:8080) are asked for their models
  (HTTP GETs to localhost only, a short timeout) when the tab opens and on Look again; Add makes one a provider.

How requests go out (Autofix's Send, Test connection): a server on this PC with a few lines of HTTP/1.1 over
`std::net` (loopback only); anything else through `curl`, with the key in curl's config on its stdin (never in its
arguments or in a file). Claude: `POST <base>/v1/messages` (`anthropic-version: 2023-06-01`; Sonnet and Opus with
server-side `fallbacks: "default"` on a refusal; Haiku has none); OpenAI and compatible servers: `POST
<base>/chat/completions`. No TLS stack in the app (a Rust HTTPS client would add about 1-2 MB and a dozen crates);
`curl` is on every Arch system. Demo mode reads, writes and sends nothing.

## Autofix (Setup > Autofix)

A language model reads the redacted findings and logs, explains what looks wrong and may propose fixes, but only from
the catalogue above: its answer may only hold `EXPLAIN <text>` and `FIX <id> arg=value` lines
(`pimax_device::autofix::parse_reply`; at most 5 fixes; anything else is shown greyed as rejected). Each fix is
approved or declined one at a time and runs like a Setup fix (root only through the pkexec dialog), logged with its
undo.

1. **Provider**: the one the AI tab picks for Autofix (Change in the AI tab), with where its key comes from.
2. **What will be sent.** Exactly the text of the request, read-only, with its size: the instructions, the fix
   catalogue and the bundle (system lines, the findings as JSON, the end of monado.log and the other session logs),
   redacted like a problem report, and every key the app knows removed.
3. **Send** is the only network call: Claude with `max_tokens` 16000 and effort high, OpenAI-style servers with a
   4096-token answer. The app log gets one line per request (size, provider, model), never the content. Demo mode
   answers itself.
4. **Contribute.** A local branch `contrib/<date>-<headset>` with `reports/<date>-<headset>.toml` (the test report of
   ARCHITECTURE.md 6.2) and a `PATCH.md`, both redacted: written to `~/.local/state/pimax/contrib/`, then committed
   through a temporary `git worktree` when you press Make the branch (your checkout is not touched). The push, the
   `gh pr create` and the `git format-patch` lines are printed for you to run; nothing is uploaded.

## WayVR (Desktop / WayVR row)

- **Now** (WayVR running): Reset screens (`wayvrctl reset-screens`), Show all / Hide all (`switch-set 1` / `0`),
  Show / hide all (`show-hide`), the captions and the wrist readout panels (`panel-modify ... set-visible`), the apps
  inside WayVR (`process-list`, stop = `process-terminate`, start = `process-launch ... floating`). Each call runs in
  the background with a 3 s timeout (WayVR's request/response commands have none).
- **Next desk start**: grab screens into the room, the A/X pin, keep the apparent size, the WayVR keyboard, the gaze
  pointer, the grab hint, push/pull speed, screen width and distance limits; the captions screen (Voice row). Saved to
  `~/.config/pimax/wayvr-desk.conf`; `pimax desk start` writes it as `conf.d/50-pimax-app.yaml` in the desk WayVR home
  (after the launcher's own `30-worldlock.yaml` and `35-desk.yaml`, before a preset's `60-preset.yaml`): generated
  whole, every key once, validated (WayVR panics at start on a duplicated key, a null value, `min > max` of the grab
  limits or a non-finite number). The captions screen reaches WayVR as `PIMAX_CAPTIONS_SCREEN`. WayVR reads its
  config once: saved while desk mode runs, it says "restart desk to apply". The app never writes into conf.d and never
  touches `~/.config/wayvr`.
- **Config check** of the desk WayVR conf.d, the way WayVR reads it: a key twice in one YAML file (what crashed WayVR
  on 14 Oct), `key:` without a value, a second YAML document, a key and its old alias in two files, broken or empty
  JSON (a `zz-saved-*.json5` cut off by a crash), capital extensions, ignored files, and the in-VR dashboard's
  `zz-saved-config.json5` (it wins over the app for its keys).
- **Saved screen layout** (`zz-saved-state.json5`): the screens with their mode, position and width; Back up and Reset
  (backup first, outside conf.d in `~/src/desk-out/wayvr-state-backups`; reset only while WayVR does not run, because it
  rewrites the file when it exits).
- **Controls**: the numbered items of [WAYVR-CONTROLS.md](WAYVR-CONTROLS.md), and the full guide.

## Room (Room row)

The anchor hook switch adds `--anchor` (`PIMAX_ANCHOR=1`) to the next start; Monado then keeps its head poses for the
room tools and applies the world offset they write, so the windows come back to the same real spot.
**Map my room** runs `pimax-roommap build --source broker --seconds N --name NAME` with the walking instructions
(slowly through the room, walls and fixed things, come back past the start: that closes the loop). **Recognise room**
runs `pimax-roommap relocalize --source broker --name NAME --apply`. The AprilTag: `pimax-anchor calibrate`, `apply`,
`clear`, and the printable tag. Both need a desk session with 6DoF, the broker and the hook (the buttons say what is
missing). Only one of the room map and the tag writes the offset per Monado session: the app records the writer next
to the offset (`$XDG_RUNTIME_DIR/pimax-anchor.offset.owner`, `roommap <pid>` / `anchor <pid>`), refuses the other one
until Clear offset, and checks again right before the tool writes. An offset written outside the app also blocks.
**View room map** (needs only a saved map, no session) runs `pimax-roommap view --name NAME --open`: it writes ONE
self-contained HTML5 page `~/.local/share/pimax/roommap/NAME/map.html` (WebGL, no network, data inlined) and opens it
with `xdg-open`. The page shows the points coloured by height (floor to ceiling) or by how often they were seen, the
headset path with a dot per keyframe, a 1 m floor grid, a top-down floor-plan toggle with scale bar, a point-size
slider and the stats (points, keyframes, mapped area in m); orbit/pan/zoom with mouse or touch, dark mode follows the
system. Points of all keyframes are merged on a 2 cm grid (the count is the times seen). `pimax-roommap export
--name NAME [--out FILE.json]` writes the same data as JSON (points x,y,z,n; keyframes position + quaternion x y z w;
floor; anchors; metadata). The floor drawn is `floor.toml` when stored, else detected from the map for the picture
only (nothing is saved).
Background: [analysis/room-map.md](../analysis/room-map.md), [analysis/anchor/README.md](../analysis/anchor/README.md).

## Auto mode (Auto mode row)

The app talks to `pimax-autod` over its socket (`$PIMAX_AUTOD_SOCKET`, else `$XDG_RUNTIME_DIR/pimax-autod.sock`;
[AUTO-MODE.md](AUTO-MODE.md)) in the background every 2 s while the row is open: the state (unplugged ... active ...
sleep), dry run or live, paused; the switch pauses or resumes it; Desk or Game, the game runtime (Monado + xrizer, or
SteamVR started by hand), the preset; Start now (`wake`) and Sleep now (`sleep`). `set-mode` lasts until the daemon
restarts. Without the daemon the row shows how to run it. **Pair left / Pair right** (controller cards and this row):
one StartPair per click through the daemon's `pair` command (approved 2026-10-14, LESSONS row 11), "Press and hold the
controller's pairing buttons (see the Pimax manual)", a 35 s progress, the result; without the daemon the same through
`pimax auto pair <side>`. At most one request per hand every 30 s (the button says when the next one is possible).
Demo mode only simulates it and sends nothing.

## Stability: crashes, hangs, reports

Why it "kept crashing": eframe ends its event loop when the OpenGL buffer swap fails (seen with NVIDIA EGL on Wayland)
and reports it only through the `log` crate, which had no backend: the window vanished without a word. A vsynced swap
also blocks while the window is on a hidden workspace (no frame callbacks), and the desktop then calls it "not
responding". What the app does now:

- **Supervisor.** `pimax-setup` is a small process without graphics that holds the single-instance lock (a second
  start brings the window to the front), starts the window process, and restarts it after a crash (at most 3 times in
  2 minutes, then a desktop notification). Logout, SIGTERM and Ctrl+C end both; "terminate" in the not-responding
  dialog on a hung window restarts it; a plain SIGKILL is respected.
- **One renderer by default: OpenGL (glow) on Wayland, without vsync.** A failed GL buffer swap still ends eframe's
  window, but the supervisor brings it back within a second on the same page, and without vsync the swap no longer
  waits for a hidden workspace. Two cargo features add fallbacks for machines where that is not enough:
  `--features wgpu` (Vulkan first: egui-wgpu recovers lost or outdated surfaces instead of ending; glow when it cannot
  start; a renderer that fails twice while running is swapped; +5.6 MB) and `--features x11` (XWayland as the last
  resort when no Wayland connection works; +1 MB). The package and the default build have neither.
- **No vsync, own frame limit**, and nothing on the UI thread touches the machine: the live state and the facts come
  from the monitor thread, every on-demand read (log tails, the docs, the launcher's plan, WayVR and room files,
  terminals, SIGINT, profiles, the app state, the auto mode socket) runs as a background task. The startup check runs
  in the background too ("checking this PC..."; no start before it is done).
- **Log**: `~/.local/state/pimax/logs/pimax-setup.log` (both processes; rotated at 5 MB, 5 kept, at most 20 MB per
  process), the window's stderr in `window-stderr.log`, every action's output in `jobs/`.
- **Crash reports** in `~/.local/state/pimax/crashes/<time>-<kind>/report.txt`: panic (message, source location,
  backtrace), native crash (signal, the window's stderr and last breadcrumbs), renderer error, hang (the watchdog
  writes the thread states when the UI thread does not answer for 10 s), kill, unexpected exit; each with the version
  (git commit, build date, ELF build id), OS, kernel, GPU driver, desktop, renderer, page, running action and the log
  lines before it. A native crash and a fatal panic also get a `minidump.dmp` written by the supervisor (all threads,
  the loaded modules with their build ids). A page that panics shows an error card; the rest of the window goes on.
- **Stripped binaries.** Release builds are stripped (workspace profile), so backtraces show no function names. What
  stays useful: the panic message and its source location (compiled in), the build id, and the minidump. The Arch
  package is built with line tables and `options=(debug)`, so makepkg keeps the symbols in `pimax-linux-git-debug`
  (`/usr/lib/debug/.build-id/..`); with it installed, `minidump-stackwalk --symbols-path` (rust-minidump) or
  `eu-stack`/`addr2line -e /usr/lib/debug/.build-id/<id>.debug` resolve a report's frames. A build from the checkout
  (`cargo build --release`) has no symbols anywhere; rebuild with `CARGO_PROFILE_RELEASE_STRIP=false
  CARGO_PROFILE_RELEASE_DEBUG=line-tables-only` to debug a crash you can reproduce.
- **After a crash** the window opens with a dialog: what happened, what still runs (a desk or VR session keeps running
  without the app, with Stop / SIGINT buttons), the report (copy, open folder), Report a problem.
- **Report a problem** writes `~/.local/state/pimax/reports/pimax-report-<time>.tar.gz`: summary, doctor report
  (text and JSON), checklist, preflight, GPU kernel events, the app log, the end of the session logs, the last action
  outputs, the crash reports; minidumps only when ticked. Every text is redacted first (home directory, user and host
  names, headset and eye chip serial numbers, machine id, e-mail, IP and MAC addresses, file system UUIDs). Nothing is
  sent anywhere. Uploading is designed but not built: opt-in, off by default, one click per report with the file shown
  first, no endpoint configured (`pimax-gui-core/src/upload.rs`).

## Tests

`cd rust && cargo test -p pimax-gui-core -p pimax-setup` (no hardware, nothing started):

- unit tests of the core (preflight rules on fixture machines, checklist, status parsing, job ordering, safety rules on
  every command the app builds, crash reports, redaction, the problem report archive, the WayVR conf.d check and
  layout, room offset ownership, the auto mode protocol, headsets, devices; a demo onboarding over a temporary state
  directory, Autofix sending nothing before Send (a transport that counts calls), root fixes waiting for the pkexec
  confirmation, the loopback HTTP client against a local server, key file rules, the doctor's commands starting once
  per round, fixes.log and Undo);
- **e2e** (`tests/e2e.rs`, egui_kittest): the real window in demo mode with its monitor and task threads, on a machine
  whose every access is slowed down and recorded from the UI thread; every page, every Home row opened, every button,
  checkbox, radio button, tab and drop-down clicked (297: every Setup tab including the demo Autofix answer, every
  onboarding step), text typed, dialogs answered, the window resized. Asserts: no machine access and no file read from
  the UI thread, no frame over 50 ms (release), no panic, nothing started. Release build: 814 frames, slowest 6.9 ms
  (all eleven rows opening at once), p99 1.7 ms, mean 0.29 ms;
- **supervisor** (`tests/supervisor.rs`, the real binary, no display): no display (both renderers tried, then a
  report), panic (4 reports, gives up after 3 restarts), SIGSEGV (minidump of the window process), hang (watchdog
  report, kill from outside restarts it, SIGTERM ends both), single instance, unexpected exit. The deliberately
  crashing child sets RLIMIT_CORE to 0 and, once its minidump is written, makes itself non-dumpable, so the tests
  leave no system core dumps (coredumpctl);
- **soak** (`tests/soak.rs`, ignored by default): `PIMAX_SOAK_SECS=1800 PIMAX_SOAK_MODE=idle|resize cargo test --release
  -p pimax-setup --test soak -- --ignored --nocapture`. Results of the 30-minute runs below.
- screenshots of every page (`tests/pages.rs`).

Soak results (30 minutes each, release, demo data with a live desk session, a machine access delay of 1 ms):

| run | frames | slowest | p99 | mean | memory (warm -> end) | threads | UI-thread machine access / file reads |
|---|---|---|---|---|---|---|---|
| idle (a frame every 100 ms) | 17,960 | 1.45 ms | 0.36 ms | 0.23 ms | 16.8 -> 17.0 MB | 4 | 0 / 0 |
| resize (new size every frame, a page every 5 s, 378 clicks) | 109,244 | 5.11 ms | 1.49 ms | 0.56 ms | 23.3 -> 25.6 MB (the test keeps every frame time) | 4 (5 for a moment) | 0 / 0 |

Idle cost on this machine with the real background checks (the window draws about one frame per second; the live
status reads /proc every second, the facts every 10 s, the doctor every 60 s): 0.5 % of one CPU core, 36 MB resident
(90 s, `PIMAX_SOAK_REAL=1 PIMAX_SOAK_STEP_MS=1000`). The GPU draws one small OpenGL frame per second when idle; not
measured with a window (none could be opened while the headset was in use).

## How each LESSONS row is enforced

| # | Check, ordering step or state |
|---|---|
| 1 | Every start (desk and the tool sessions) is a job whose first step waits until no `monado-service` is left (`pgrep -x` equivalent over /proc, 45 s, progress every 5 s, then a message with the pid and what to do). The preflight blocks while a session, a foreign monado-service or session leftovers run, with "Stop session" (`pimax desk stop --grace 30`) or "Stop it" (`pimax vr stop --grace 30 --all`). A stop job reports the launcher's exit code (it is 2 whenever this boot has any Xid line) and then checks that Monado is gone. The app's own only signal is SIGINT to one pid after a confirmation, with the name and the start time checked again, only on the real /proc, never to monado-service, pid 1 or itself. |
| 2 | Holders of the Pimax `/dev/video*` nodes (open file descriptors under /proc) block a start that uses the cameras: "Stop session" when it is the session's broker, else "SIGINT pid". Checked again right before the launcher is called. 6DoF and controller tracking go through the launcher's broker (started first, stopped last); controller tracking is only offered with 6DoF; the controller recording starts its broker first and stops it last; the walk test needs the cameras free. |
| 3 | A holder of the eye chip (usbfs node of 2104:0561) blocks a start with eye tracking. Eye view, IPD and eye calibration are greyed out while Monado runs with `PIMAX_EYE=1`, with "Restart desk without eye tracking" (stop, wait for Monado to exit, start without `--eye`). The live status warns when monado.log says the chip was held by another process. |
| 4 | Lens test and eye calibration are main OpenXR apps: they need no other Monado and get their own session. OpenXR clients left from earlier runs are listed with a SIGINT button. The hands test is a headless client. Desk mode order (passthrough as the main app, WayVR overlay) is the launcher's. |
| 5 | The app never starts monado-service: desk mode and the walk test are `pimax desk start ...`, tool sessions `pimax vr start --monado-build ...`, all through the launcher's env builder. The plan (which build, which clients) is computed with the launcher's own `parse_desk`/`plan`. A switch the installed launcher lacks blocks the start instead of being dropped (no silent 3DoF). Experiment variables of the launcher's watch list are removed from its environment. |
| 6 | `PIMAX_CAM:BOOL=ON` in the `CMakeCache.txt` of the build the launcher will use, for 6DoF and controller tracking (blocking); Setup item per build; "Rebuild Monado (desk)" reconfigures and rebuilds; the live status flags `built without -DPIMAX_CAM=ON` from monado.log. |
| 7 | `getcap` of every build (desk, slam, pimax); root fix `sudo setcap cap_sys_nice+ep <path>` (Copy, pkexec); the setcap dialog opens after every rebuild; the preflight warns for the chosen build. |
| 8 | Installed launcher vs workspace build vs sources. The install fix runs only while nothing of the stack runs (installs next to the target, then renames). |
| 9 | The kernel log of this boot is classified line by line: only Xid 31 from the process named exactly `wayvr` is yellow and the start needs "Start anyway" (adds `--force`, offered only when nothing else fails, because `--force` skips all launcher checks; the same for tool sessions). Right before a `--force` start the job re-reads the kernel log, the USB headset, the keepalive, the connector and merged_head_sync and aborts if anything changed. Any other Xid, an Xid without a process name, "Idling display engine" or a stuck channel is red: start blocked, reboot recommended. An unreadable (empty) journal is reported, not taken as clean. The doctor's kernel-log item is mapped the same way. WayVR headless renders are refused by the safety rules. |
| 10 | `merged_head_sync` must be 1 (else blocked). Setup: `modinfo nvidia-modeset` lists the parameter, both pacman hooks are installed (root fix: the INVENTORY 3.4 command with absolute paths). DRM hotplug, sysfs/debugfs writes and firmware tools are refused by the safety rules. |
| 11 | The app sends no HID: it only calls the tools. The safety rules refuse `--with-cal`, `--force-replay` and `video-on`. The IPD tool says that `a` sends the approved motor command once; the power-on tool runs the approved enable through `pimax-eye power-on`. |
| 12 | The chosen build must contain the Basalt IMU gate (marker string of Monado slam d3138d4) for 6DoF, which turns auto exposure on; the walk test needs it when AE is on. |
| 13 | Preflight note for 6DoF and the walk test: lit room, cameras free, not on a cushion. |
| 14 | The default switches are eye + 6DoF (the command of the first good live result); `--force` only with consent. |
| 15 | Free disk space in Setup; the walk test raw recording and the controller recording need twice their estimated size (2.2 GB per 45 s). |
| 16 | The builds list with branch; the desk build is the main one. The app runs no git command. |
| 17 | Logs page sources as listed above. |
| 18 | `OXR_DEBUG_IPD_MM` from `~/.config/pimax/ipd.env` is put into the launcher's environment, so Monado and its desk clients get it; the eye calibration file is shown in Setup. |
| 19 | With voice on, and in the voice test when nothing runs: the headset microphone sends zeros while the display is off. |
| 20 | Tools > Steam runtime: `vr_runtime_switch.sh xrizer\|steamvr\|status` with the desk build's runtime file, the `chmod 444` fix, the launch options with Copy. `steam://` and `-applaunch` are refused. |
| 21 | Root only through `pkexec /bin/sh -c '<the documented lines>'`, after a dialog that shows that exact command; the lines may only use the documented programs (setcap, install, udevadm, tee, echo, patch, dkms, mkinitcpio, sed on nvidia-pimax.conf, mount by UUID, the reapply script). Package installs, reboots and the EDID override (which names this PC's PCI address) are copy-only. With a polkit authentication agent running, the fix runs as a job (output in the app); without one (this Omarchy install has none), the same pkexec command opens in a terminal, where pkexec asks for the password. The shared NTFS drive item (only where its UUID exists) mounts by UUID. |

## Hard rules in code (`rust/pimax-gui-core/src/safety.rs`, tested)

Every command passes `safety::check` right before it starts (jobs and terminal scripts): no `pkill`/`killall`/`fuser -k`,
no SIGKILL in any spelling, nothing under `/sys` or `/proc/sys`, no `edid_override` or debugfs, no firmware tools, no
Steam game launches, no eye calibration ops or forced replays, no Hyprland config, no WayVR headless renders, no recursive
deletes. The checker looks through variable assignments, shell keywords and wrappers (`env`, `timeout`, `flock`,
`xargs`, `nohup`, ...), checks nested `sh -c` scripts, and refuses command substitution, backticks and `eval` so that no
command is hidden from it. User commands never elevate (`sudo`, `pkexec`, `su`, `doas`, by any path), never touch `/etc`,
`/usr/src`, `/boot`, DKMS, initramfs, modprobe or udev, and use `systemctl` only with `--user` (`systemd-run` only as
`--user --scope`). Root commands are `/usr/bin/pkexec /bin/sh -c` with the documented fix lines only: an allowlist of
programs, writes only to the documented destinations, no `PATH=`/`LD_*=`, the NVIDIA script from the root-owned
`/usr/local/lib/pimax` copy when it exists. The read-only commands of the checks (`journalctl`, `getcap`, `modinfo`, the
doctor's `patch --dry-run`, `pimax desk help`) run from an allowlist with a 20 s timeout. Stopping is
`pimax desk stop --grace 30` or `pimax vr stop --grace 30`; processes are signalled only by pid. The checker guards
against mistakes in the app's own command builders (every builder is tested against it); it is not a sandbox.

## Design

- `rust/pimax-gui-core` (no UI): `facts` collects the live state and the facts on the `monitor` thread; `worker` runs
  every on-demand task off the UI thread; `preflight` and `checklist` are pure functions of the facts; `tools`, `wayvr`,
  `room`, `auto`, `headsets`, `devices` describe the rows and cards; `job` runs ordered steps (wait for Monado,
  re-check, run) one at a time with output through a file; `term` opens terminals; `cmd`/`safety` build and check
  commands; `applog`, `crash`, `bundle`, `redact`, `orphans`, `instance`, `upload` are the stability parts; `app::AppCore`
  holds the state and every action. The machine is behind a `Machine` trait (the doctor's `Host` plus symlinks,
  metadata, free space), with an in-memory fake for tests and demo data, and `probe::ProbeMachine` for the UI tests.
  Reused as libraries: `pimax-tools` (desk plan, the WayVR settings file, log parser, env watch list, Steam options),
  `pimax-doctor`, `pimax-config`.
- `rust/pimax-setup` (UI): eframe/egui 0.36, `supervisor` + `window` processes, `theme` (palette, fonts, eased
  animations), `widgets` (rows, pills, device cards, buttons), `pages/*`; `screenshot.rs` renders egui frames on the
  CPU for `--screenshots`.
- Size: the stripped release binary is 11.6 MB (glow on Wayland, crash handling, the core); 17.2 MB with
  `--features wgpu`, 12.6 MB with `--features x11`; it was 20.9 MB unstripped with both renderers and X11.
  `opt-level = "s"` for the GUI crates would give about 10 MB; not set (the workspace profile is shared). Direct
  dependencies carry no default features beyond what they need (eframe without its defaults, the icon font as a
  compile-time subset, flate2 with the Rust backend); the rest (winit, glutin, smithay-clipboard, arboard with its
  image support) comes with eframe's clipboard and Wayland support.

## Not done / limits

- Not tried on this machine with a window yet (a VR session ran while it was built): the supervisor, both renderers
  and the hidden-workspace behaviour were tested headless and by tests, not by hand on Hyprland.
- Voice models and Basalt have no automatic install (their download source is not recorded); Setup shows what to put
  where. A Monado build directory that was never configured is not configured by the app.
- Starting and stopping desk mode, the room tools, WayVR's live actions and pairing from the app were not exercised
  against the headset by the agent that wrote them; the app runs the same commands as the terminal.
- Autofix and Test connection were not tried against a real model here (no local server ran and no key was used): the
  requests are tested against a local mock server and the Claude / OpenAI request and answer shapes against recorded
  JSON. Contribute's branch job was
  not run against the real checkout (demo mode and tests only).
- The manual steps the app does not cover yet are listed in [MANUAL-STEPS.md](MANUAL-STEPS.md) ("GUI coverage gaps").
- Controller battery is not reported by Monado's log: shown as unknown. Brightness, foveated rendering and video
  recording are mocks.
- NVIDIA only: on other GPUs the NVIDIA checks are skipped, nothing else is adapted (see
  `analysis/research/portability-and-calibration-gui.md`). Other Pimax headsets are mocks.
