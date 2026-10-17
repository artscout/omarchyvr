# One click to VR: Play and Work, the Monado cap, start at login, the hand test

Everything here is in the Pimax app (`omarchyvr-app`), Home page, the headset card. Code: `rust/omarchyvr-gui-core/src/play.rs`,
`rust/omarchyvr-gui-core/src/handtest.rs`, UI in `rust/omarchyvr-app/src/pages/home.rs`.

## Readiness line

Above the buttons: **Headset** (on USB), **Monado cap** (`cap_sys_nice` on the monado-service the next start uses, from the
launcher's own plan), **OpenXR runtime** (`~/.config/openxr/1/active_runtime.json` points at that build; the start
sets it anyway) and the **last start** from the app (ok / failed, how long ago).

## Play and Work

Three layers: the **base** Pimax services (Monado + driver, tracking, controllers, hands, recording, keepalive,
omarchyvrd; their own switches, shared by both sides), **Play** and **Work**. Home's headset card has two big buttons:

- **Play**: games, like SteamVR. `omarchyvr vr start` with the base switches (6DoF, controllers, hands, anchor, readout,
  refresh rate) for Steam (xrizer) and OpenXR games; when the auto mode daemon runs it is told
  `set-mode game --runtime monado|steamvr`. It never starts WayVR or the desk AI, and before it starts anything it
  stops (by pid) what a crashed Work left and every local AI process, so the game has the whole graphics card; Home
  then shows what it stopped and, while Play runs, what uses the graphics card (docs/GAME-MODE.md,
  docs/PLAY-WORK-AUDIT.md). Eye tracking in Play only with the switch "Eye tracking in games" (off by default). The VR
  home/launcher it will also start is still to be decided.
- **Work**: the WayVR desk on top of the base: `omarchyvr desk start` with the base + desk switches (screens,
  passthrough, voice, AI, captions, presets, eye tracking). The first Work press asks the Work questions first (voice
  assistant, Lara, speech; docs/WORK-FIRST-RUN.md) and starts Work once they are answered; later presses start at once.
  In the headset, the left menu button opens the Work menu with **Exit Work** (docs/WAYVR-CONTROLS.md).
- While our session runs, the button of the running side is **Stop Play** / **Stop Work** and the other one is
  **Switch to Work** / **Switch to Play**: one job that stops the running side completely (Work's stop takes the
  local AI, voice and the VR screens with it), waits until Monado is gone, then starts the other side.
- Same gate as before: the preflight must be ready (or "Start anyway" ticked for WayVR Xid 31 faults only).
- The preflight refuses a Monado build that lacks any feature marker (PIMAX_SLAM, the IMU gate, PIMAX_EYE,
  PIMAX_CTRL_TRACK, PIMAX_ANCHOR, and PIMAX_HT_HZ when camera hands are on) and says which one in plain words.
  Setup > Builds lists each feature of the one build as ok / missing.
- **One saved setting** per side: the app and the CLI share it. Every start (app, `omarchyvr desk|vr start`, omarchyvrd, a restart)
  records its flags in `<session dir>/start-argv.txt`; the app takes a newer one over as its switches (a `vr` start changes
  only the base switches), so the next Play or Work repeats the last session. (The hand test's own start is not taken over.)
- Fresh defaults are the normal setup: eye, 6DoF, optical controllers, passthrough, voice, room anchor, the wrist
  readout, 90 Hz (72 Hz stays selectable; a saved choice is kept).

## The Monado cap (cap_sys_nice)

Every relink of monado-service drops it (LESSONS row 7). When the build Play uses lacks it:

- the readiness line shows **Monado cap missing** with a **Fix** button;
- **Play** and **Work** fix it first by themselves.

Both run `/usr/bin/pkexec /usr/bin/setcap cap_sys_nice+ep <service>`: one graphical password window (polkit agent),
then the app checks again. Nothing is installed (no sudoers or polkit rules, no helper). Before pkexec the path is
resolved and must be a regular file named `monado-service` under `~/src/monado*/`; the safety rules accept only this
exact command shape. A cancelled password window does not stop the start (the session starts without the priority).
Without a polkit agent, Fix opens the usual root-fix dialog instead. The CLI prints the same warning as before and
says that the app can fix it.

## Start VR when I log in

A switch under Play, **off** by default, saved in `~/.config/omarchyvr/gui-state.toml` (`autostart`). When the app opens
(it autostarts at login) it starts the last side you used (Play or Work) by itself only if the headset is connected (it waits up to 40 s for it), the
Monado cap is present (never a password window at login), and the preflight gate is clear (no reboot needed, no GPU
errors, no consent needed). Otherwise Home says why ("Not started at login: ...").

## Camera hands

Tracking row: **Camera hands** (`--hands`) and **Hands replace controllers** (`--hands-roles`). One build carries
everything: `--hands` only sets the Mercury variables on the build the start uses. Play and Work pass them on. The switch is greyed out, with the
reason, when the launcher has no `--hands`, the build lacks `PIMAX_HT_HZ`, or the models are not installed.

## Guided hand test

Tracking row > **Start the hand test**:

1. (Re)starts desk mode with `--6dof --hands --voice` (the voice flag gives the captions panel).
2. Talks you through it **in the headset** on the captions panel under the main screen (the live captions are
   paused meanwhile): get ready 8 s, left hand 10 s, right hand 10 s, both 10 s, open/close and pinch 10 s. There is
   no spoken voice (no TTS engine installed) and no desktop notification.
3. Records `pimax-cam record ~/src/cam-data/hands-test-<time>.pxcam --seconds 40 --source broker` during the 4 steps.
4. Runs `nice -n 19 <build>/src/xrt/drivers/pimax_ht_bench <file>` and shows its `detection:` line, plus the last
   `hands (running)` lines of monado.log, on Home. The session keeps running.
