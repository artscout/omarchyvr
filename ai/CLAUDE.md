# omarchyvr: rules for every agent

Goal: the Pimax Dream Air works on Linux like Pimax Play on Windows: **one app, one click, everything is part of one whole.**
Read this before any work. It applies to the main thread and to every subagent.

## 1. GUI first: no command line for the user
- The user does not use the command line. Everything is reachable from the Pimax app (`omarchyvr-app`, autostarts at login).
- Never answer the user with a command to run. Do it yourself, or add a button/toggle/page to the app.
- Root steps (setcap) go through a GUI password prompt (pkexec) from the app, never "run sudo ..." in chat.
- Guided tests (hand test, controller test, pairing) run from the app and talk to the user **in the headset**
  (voice/in-VR text). Desktop notifications are invisible while wearing the headset.
- The CLI (`omarchyvr`, `omarchyvr-voice`, `omarchyvr-record`, ...) stays as the engine behind the app and for agents/tests.

## 2. Definition of done (a feature is not done until ALL are true)
0. **A switch in the app** (on/off, plus its options) is the only way the user turns it on; the setting is saved and shared with the CLI.
1. **Built** in the one canonical pipeline (section 3), not in a side build only.
2. **Verified build**: the Monado binary still has every marker (section 3). A build that loses one is a regression.
3. **In the app**: a control (toggle, button, card or page), its status, and its errors shown in plain words.
4. **In the session**: started/stopped by the app's Play button with the saved settings; recorded in start-argv.txt so restarts keep it.
5. **Reliable**: a omarchyvrd component (leaf if optional). If it fails, VR keeps running with zero missed frames.
6. **Tested**: cargo tests and goldens pass; the screenshot of its app page (`omarchyvr-app --screenshots DIR`) looked at.
7. **Documented**: docs/<FEATURE>.md plus a line in the app's help.
8. **Installed only between sessions** by the main thread from a clean build of committed HEAD.

## 3. The pipeline: one whole build
**Principle (user, 2026-10-15): everything is in the same code and the same build; the app only has switches.**
Every feature is compiled into the one build of every component. Only the user's settings (switches in the app)
decide what is active. No feature needs a different build, build dir, flag-only CLI path or a rebuild to turn on.
Every change ends in a new complete build of the whole set (Monado + WayVR + all Rust binaries + the app), installed together.

- **Monado**: ONE build that carries everything: Pimax driver, 6DoF/SLAM, eye, controller tracking, anchor, recorder tap,
  camera hands. Branches are merged into it; no feature lives in a build of its own.
  - Required CMake options (copy from the working build, never start a fresh cache):
    `XRT_BUILD_DRIVER_PIMAX=ON`, `PIMAX_CAM=ON`, `XRT_HAVE_SYSTEM_CJSON=OFF` (+ ONNX Runtime for hands).
  - Required markers in `monado-service` (check with `grep -ac`): `PIMAX_SLAM`, `PIMAX_ANCHOR`, `PIMAX_EYE`,
    `PIMAX_CTRL_TRACK`, `held from the tracker` (IMU gate), plus each new feature's marker. The app's preflight
    (omarchyvr-gui-core builds.rs / preflight.rs) refuses builds without them.
  - Lesson 2026-10-15: build-rec was configured from scratch with the Pimax driver OFF; the app blocked 6DoF.
    Always copy the cache options from the live build and check the markers before switching `~/.local/lib/omarchyvr/monado`.
  - Every relink drops cap_sys_nice: the app's Fix/Play button restores it (pkexec).
- **Rust** (`rust/`): build the whole workspace from a clean worktree of committed HEAD
  (`CARGO_TARGET_DIR=rust/target-clean`), then install all binaries together. No half-installed sets.
- **WayVR** (`~/src/wayvr`, branch eye-gaze) is part of the same set; local commits only.

## 4. Safety rules (unchanged)
- Never touch a live VR session or its processes; install only between sessions. Kill by PID only.
- No sudo by agents. No firmware flashing, no DRM hotplug, HID bytes only from Windows captures.
- Don't edit hypr, systemd system units, NVIDIA or boot config, or `~/.config/wayvr`.
- Never tell the user to unplug the headset, with ONE exception (user decision 2026-10-17): during setup the app may ask the user to power-cycle the headset's POWER cable (unplug about 10 s, plug back in, DisplayPort stays in) as the LAST resort, only when no VR session runs and after the gentler steps failed (press the power button, wait 90 s). Never during a session, never the DisplayPort cable during a session, never as a first step. Lesson 40.
- Monado and WayVR: local commits only; never push to upstream. They are published only by the export step to the PRIVATE forks laughtt/monado and laughtt/wayvr (branch `omarchyvr`, clean copy, noreply identity; user 2026-10-16). omarchyvr: commit only your own files, push origin main. Private context (lab notes, data, machine facts) lives in the untracked `.claude/` folder and `CLAUDE.local.md`: never commit or push them.
- Heavy work (builds, tests, benches, AI audits) runs at `nice -n 19` with `-j8`; never send keys/typing to the user's windows.
- No AI audits, benchmarks or GPU tests while `monado-service` runs (check `pgrep -x monado-service` before and during every run): they share the GPU and made VR drop 8% of frames (shaky screens, 2026-10-16 18:08).
- Voice commands run without confirmation by default (`voice_confirm = none|dangerous|always`).
- The Mercury hand models have no licence: personal use only, never redistributed.
- Commit trailer: `Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>`.
