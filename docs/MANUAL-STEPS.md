# Manual steps for the Pimax Dream Air Linux stack

Every command and step we use to run the stack, so the Pimax GUI can take them over. Paths are relative to `~/pimax-re` unless absolute. Entries marked "unverified" come from notes, not from a run I could check.

GUI column: yes / partly / no, from docs/GUI.md. "partly" means the GUI detects or offers the step but does not do all of it.

## 1. One-time setup

### 1.1 EDID override and NVIDIA modeset config (root, then reboot)
```
sudo install -D -m644 linux/pimax-dp-edid-native.bin /etc/pimax/dp-edid.bin
echo 'override.[0000:01:00.0].DP-0=/etc/pimax/dp-edid.bin' | sudo tee /etc/pimax/nvidia-modeset.conf
echo 'options nvidia-modeset config_file=/etc/pimax/nvidia-modeset.conf' | sudo tee /etc/modprobe.d/nvidia-pimax.conf
echo 'FILES+=(/etc/pimax/nvidia-modeset.conf /etc/pimax/dp-edid.bin)' | sudo tee /etc/mkinitcpio.conf.d/pimax.conf
sudo mkinitcpio -P && reboot
```
- What: puts the headset EDID (native modes, 8 bpc) into the initramfs so NVIDIA reads it at boot.
- When: once, and again if the EDID file changes.
- Root: yes.
- Source: linux/DISPLAY.md (Setup).
- GUI: partly. Setup > Install shows which EDID is installed (non-desktop, native or another) and the doctor's connector lines, copy only.
- GUI should: stay copy only. The lines name this PC's PCI address and DP index, and a wrong line leaves no display at boot.

Non-desktop EDID (lets Hyprland lease the panel for Monado direct mode): `linux/pimax-dp-edid-msvsdb.bin`, copied to `/etc/pimax/dp-edid.bin` in the same way, then `mkinitcpio -P` and reboot. Remove the DP-1 line from `~/.config/hypr/monitors.lua`. Verified 2026-10-13 (linux/DISPLAY.md, "Non-desktop").

### 1.2 Hyprland monitor line
```
hl.monitor({ output = "DP-1", mode = "7104x3840@72", position = "8000x8000", scale = 2 })
```
- What: keeps the headset off the main screen.
- Root: no. Source: linux/DISPLAY.md step 3.
- GUI: partly. Setup > Install reads `~/.config/hypr` and shows the line to add, or to remove when the non-desktop EDID is installed.
- GUI should: stay read only. The app never edits Hyprland's config (safety rule).

### 1.3 udev rule
Rule file in the repo: `linux/70-pimax.rules` (hidraw and USB for 34a4, Tobii 2104:0561), copied to `/etc/udev/rules.d/70-pimax.rules` on this machine. Then `sudo udevadm control --reload && sudo udevadm trigger`, or replug the headset. The packaged path is `/usr/lib/udev/rules.d/70-pimax.rules` (packaging/README.md).
- Root: yes.
- GUI: partly (doctor checks uaccess; the Setup page can apply the fix).

### 1.4 Keepalive service (user)
```
ln -sf ~/pimax-re/linux/omarchyvr-keepalived.service ~/.config/systemd/user/
mkdir -p ~/.config/systemd/user/omarchyvr-keepalived.service.d
printf '[Service]\nExecStart=\nExecStart=%%h/.local/bin/omarchyvr keepalived\n' > ~/.config/systemd/user/omarchyvr-keepalived.service.d/rust.conf
systemctl --user daemon-reload && systemctl --user enable --now omarchyvr-keepalived
```
- What: wakes the headset and sends the 213 ms keepalive. The Rust `omarchyvr keepalived` replaces the Python script via the drop-in.
- When: always on; do not stop it by hand.
- Root: no. Source: linux/DISPLAY.md; the drop-in from the transcripts; STATUS.md.
- GUI: yes. Setup > Install > Keepalive unit runs these lines as the user, only while no session runs. The doctor checks that the service is active.

### 1.5 OpenXR active runtime
```
ln -sfn ~/src/monado-eye/build/openxr_monado-dev.json ~/.config/openxr/1/active_runtime.json
```
- What: makes OpenXR apps use our Monado build.
- Automatic now: `omarchyvr vr start` and `omarchyvr desk start` set this link themselves; `--no-set-runtime` skips it. `stop` leaves it in place. `omarchyvr vr steam-options` then prints only `PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1 %command%`, which every Steam VR game still needs (LESSONS row 27).
- Root: no. Manual line only needed when you do not start through `omarchyvr`.
- GUI: yes. Every start the app runs goes through the launcher, which sets the link (checked 2026-10-15).

### 1.6 OpenVR runtime (SteamVR / xrizer)
`analysis/tools/vr_runtime_switch.sh xrizer` points `~/.config/openvr/openvrpaths.vrpath` at xrizer; `... steamvr` restores SteamVR. Undo state is in `~/.config/pimax-vr-backup/`. Once SteamVR runs it rewrites openvrpaths.vrpath itself, so re-run `xrizer` after that.
- Root: no. Source: analysis/steamvr-games.md, analysis/steamvr-native-linux.md.
- GUI: yes. Developer > VR runtime and Tools > Steam runtime (xrizer, SteamVR, status).

### 1.7 NVIDIA 2Heads1OR patches (root)
Status checked 2026-10-15 by `ls` only: the pacman hooks are installed. `/etc/pacman.d/hooks/` holds `65-nvidia-pimax-patch.hook` and `75-nvidia-pimax-verify.hook` (both dated Oct 14), next to `90-mkinitcpio-install.hook` and `99-omarchy-limine.hook`. `/usr/local/lib/omarchyvr/` holds `nvidia-patch-reapply.sh` and the three patches. STATUS.md still says "pending", which is stale. I did not verify the file contents against the templates.

Install, if it has to be redone (one command, from linux/nvidia-pimax-patch.hook header):
```
sudo env L="$HOME/pimax-re/linux" bash -c 'install -Dm755 $L/nvidia-patch-reapply.sh /usr/local/lib/omarchyvr/nvidia-patch-reapply.sh && for p in nvidia-2h1or-viewport nvidia-2h1or-flip-fix nvidia-evo-makeroom-bounded-wait; do install -Dm644 $L/$p.patch /usr/local/lib/omarchyvr/$p.patch; done && install -Dm644 $L/nvidia-pimax-patch.hook /etc/pacman.d/hooks/65-nvidia-pimax-patch.hook && install -Dm644 $L/nvidia-pimax-verify.hook /etc/pacman.d/hooks/75-nvidia-pimax-verify.hook'
```
Manual rebuild after a patch: `sudo linux/nvidia-patch-reapply.sh --rebuild` (dkms build, then `limine-mkinitcpio` on this Limine host, else `mkinitcpio -P`; then reboot). Check: `sudo modinfo nvidia_modeset` must list `merged_head_sync`, or `cat /sys/module/nvidia_modeset/parameters/merged_head_sync` prints 1. Remove: `sudo rm /etc/pacman.d/hooks/65-nvidia-pimax-patch.hook /etc/pacman.d/hooks/75-nvidia-pimax-verify.hook && sudo rm -r /usr/local/lib/omarchyvr`.
- Root: yes.
- Source: linux/DISPLAY.md, linux/nvidia-patch-reapply.sh header.
- GUI: partly (doctor checks the three patch ids; the Setup page can run fixes through pkexec).

### 1.8 Packages
- `sudo pacman -S rust` (needed for cargo; steamvr-games.md).
- `sudo pacman -S steam` (needs multilib), then Dungeons of Eternity and Proton/Experimental (steamvr-games.md; unverified whether already installed).
- `dkms` and `nvidia-open-dkms` 610.57.04 are present (transcripts).
- Build dependencies for Monado, WayVR, Basalt: see each repo's build notes (not collected here).
- Root: yes. GUI: partly. Setup > Install checks for cargo, Steam and Proton. The pacman lines are copy only: the app runs no package installs.

### 1.9 Eye tracker power-on
After a headset power cycle the Tobii module stays off until: `pimax-eye power-on` (sends the approved enable once if the chip is absent). Auto-enable is planned, not built (STATUS.md, transcripts).
- Root: no. GUI: partly (the GUI runs `pimax-eye power-on`).

## 2. After every Monado rebuild (root)
```
sudo setcap cap_sys_nice+ep ~/src/monado-eye/build/src/xrt/targets/service/monado-service
sudo setcap cap_sys_nice+ep ~/src/monado/build/src/xrt/targets/service/monado-service
getcap ~/src/monado-eye/build/src/xrt/targets/service/monado-service
```
- What: GPU realtime priority for monado-service. Every relink drops it, and runs then stutter or fail the request.
- When: after every `ninja` of a Monado build you run. The same goes for `~/src/monado-slam/build/...` (the slam build also needed it, transcripts).
- Root: yes. Source: docs/LESSONS.md row 7, STATUS.md, analysis/eye-precision.md.
- GUI: partly (the GUI checks with getcap and offers the setcap through pkexec, docs/GUI.md row 7).
- GUI should: run getcap after every build and show the setcap line when it is missing.

## 3. Installing our tools (user)

### 3.1 Launcher (on PATH)
```
cd ~/pimax-re/rust && cargo build --release -p omarchyvr-tools
install -m755 target/release/omarchyvr ~/.local/bin/pimax.new && mv -f ~/.local/bin/pimax.new ~/.local/bin/omarchyvr
```
The `.new` then `mv` step replaces the binary atomically. The keepalive service runs the installed file, so do not touch the service (omarchyvr-tools).

### 3.2 Auto-mode daemon (on PATH)
```
cd ~/pimax-re/rust && cargo build --release -p omarchyvr-autod -p omarchyvr-tools
install -m755 target/release/omarchyvr-autod ~/.local/bin/omarchyvr-autod
```
`~/.local/bin/omarchyvr-autod` is installed (ls). Install `omarchyvr` only while nothing runs (docs/LESSONS.md row 8).

### 3.3 Other tools (run from the build directory)
Not copied to `~/.local/bin` as far as I saw. Run them as `~/pimax-re/rust/target/release/<name>`, or install them the same way as 3.2.

| Tool | Build | Notes |
|---|---|---|
| pimax-eye | `cargo build --release -p pimax-eye` | eye tracker: ipd, watch, replay, power-on |
| pimax-eyecal | `-p pimax-eyecal` | needs the eye Monado build (setcap, section 2) |
| pimax-ipd | `-p pimax-ipd` | guided IPD test in the headset |
| omarchyvr-roommap | `-p omarchyvr-roommap` | room map |
| omarchyvr-anchor | `-p omarchyvr-anchor` | AprilTag anchor |
| pimax-ctrltrack | `-p pimax-ctrltrack` | controller optical tracking and gyro fit |
| pimax-cam | `-p pimax-cam` | camera broker (started by desk --6dof) |
| omarchyvr-app | `cargo build --release -p omarchyvr-app` | the GUI (docs/GUI.md) |
| omarchyvr-doctor | `cargo build --release -p omarchyvr-doctor` | read-only health checks |
| omarchyvr-passthrough | built in `~/src/target-passthrough` | passthrough main app (transcripts) |
| wayvr | `CARGO_TARGET_DIR=~/src/target-desk cargo build --release -j 12 -p wayvr -p wayvrctl --no-default-features --features openxr,osc,wayland,x11` in `~/src/wayvr` | desk overlay (transcripts) |

Sources for the table: docs/INVENTORY.md, STATUS.md, transcripts. Unverified: exact package names for pimax-eye and pimax-ctrltrack.

### 3.4 Monado builds
- Eye build: `~/src/monado-eye/build` (branch desk; needed for `--eye`, `--anchor`, `--ctrl-track`).
- Plain build: `~/src/monado/build`.
- Relink: `ninja -C <build>`, then setcap (section 2).
- Full reconfigure when Proton games fail to load Monado: `cmake -S ~/src/monado -B ~/src/monado/build -DXRT_HAVE_SYSTEM_CJSON=OFF && ninja -C ~/src/monado/build` (analysis/steamvr-games.md).

### 3.5 Auto-mode unit (user, not installed by anything)
```
install -Dm644 ~/pimax-re/packaging/systemd/omarchyvr-autod.service ~/.config/systemd/user/omarchyvr-autod.service
systemctl --user daemon-reload && systemctl --user start omarchyvr-autod
cp ~/pimax-re/packaging/auto.toml.example ~/.config/omarchyvr/auto.toml   # dry_run = true by default
```
- Root: no. Source: docs/AUTO-MODE.md, packaging/systemd/omarchyvr-autod.service.
- GUI: yes. Setup > Install > Auto mode daemon builds it and installs the unit and the dry-run `auto.toml` (the `cp` line). Developer > Auto mode then has Start and Start at login.

## 4. Daily start and stop

### 4.1 VR session (Monado only)
```
omarchyvr vr start                          # 72 Hz
omarchyvr vr start --fps 90                 # 90 Hz (mode index 3)
omarchyvr vr start --6dof --ctrl-track      # 6DoF and optical controllers
omarchyvr vr start --anchor                 # head-pose ring + world offset hook (needs desk build with hook)
omarchyvr vr start --wait                   # stay in foreground (desktop entry)
omarchyvr vr status
omarchyvr vr stop --grace 30
```
- What: preflight, then monado-service detached. Returns after "gyro bias done" (keep the headset still about 3 s).
- Root: no. Source: `omarchyvr vr help`, docs/GUI.md.
- GUI: yes (Start VR / Stop VR).

### 4.2 Desk mode (Monado + passthrough + WayVR)
```
omarchyvr desk start
omarchyvr desk start --eye                  # eye tracking (uses ~/src/monado-eye/build)
omarchyvr desk start --6dof --ctrl-track    # 6DoF, controllers
omarchyvr desk start --voice --voice-hud    # captions; voice-hud adds the old overlay
omarchyvr desk start --preset cinema|work|assistant
omarchyvr desk start --no-passthrough       # Monado + WayVR only
omarchyvr desk start --no-perf              # no performance readout on the right wrist
omarchyvr desk start --anchor
omarchyvr desk status
omarchyvr desk stop --grace 30
```
- What: `vr start` first, then `omarchyvr-passthrough run`, then WayVR as an overlay. Presets write `conf.d/60-preset.yaml`.
- `--force`: skips launcher checks. Use it only with consent: the GUI adds it only after a Xid-only warning, "Start anyway" (docs/GUI.md row 9). Cause: Xid 31 from wayvr offline renders made the preflight refuse every start for the rest of the boot (docs/LESSONS.md row 9).
- Root: no. Source: `omarchyvr desk help`, docs/GUI.md, analysis/steamvr-native-linux.md.
- GUI: yes.

### 4.3 Stopping
```
omarchyvr vr stop --grace 30       # or: omarchyvr desk stop --grace 30
omarchyvr vr stop --all --grace 30 # also stops other monado-service processes of yours
```
- SIGINT, then SIGKILL only after the grace period (30 s default). Then the kernel log check: exit 2 means reboot.
- Never SIGKILL Monado by hand; stop by PID only (STATUS.md safety rules).
- Root: no. GUI: yes.

### 4.4 Presets and checks
- `omarchyvr desk status` lists desk PIDs and `vr status`.
- `omarchyvr vr dump-stats` and `dump-diff` check colours (run only in a test).
- Logs per session in `/tmp/omarchyvr-session/` (monado.log, monado.pid) and the desk session dir (passthrough.log, wayvr.log, wayvr.stdout, voice.log, voicecap.log, voicehud.log).

## 5. Steam and games
- Print the launch line: `omarchyvr vr steam-options`. It prints `PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1 %command%` when `active_runtime.json` already points at the Monado build (set by `vr start`/`desk start`). Otherwise it prints `PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1 XR_RUNTIME_JSON=<build>/openxr_monado-dev.json %command%`, to paste under Steam > game > Properties > General > Launch Options.
- Dungeons of Eternity (appid 3189340): SteamVR mode (no flag) is the default; with the launch option above `-openxr` works too (LESSONS rows 23, 27, verified 2026-10-15). It ran end to end in an earlier test (HISTORY.md).
- For OpenXR-native games, `-openxr` is the flag some Proton games take (STATUS.md). Dungeons is not one of them.
- Source: rust/omarchyvr-tools/src/vr/config.rs (steam_options), analysis/steamvr-games.md.
- GUI: partly (the GUI shows the line; it does not write Steam's launch options).
- GUI should: copy the line, or set launch options in Steam's localconfig after confirmation.

## 6. Calibration

### 6.1 IPD
- Saved IPD: 61.5 mm, in `~/.config/omarchyvr/ipd.env` (`OXR_DEBUG_IPD_MM`). The launcher passes it to Monado at each start.
- Measure: `pimax-eye ipd --watch` (desk/VR stopped, Monado without `--eye`), keys `a`, `+`, `-`, `q`.
- Apply once (sends the approved motor command, 58-72 mm): `pimax-eye ipd --apply` or `pimax-ipd --apply --mm 61.5`.
- Root: no. GUI: yes (Optics / Display, Apply, "Measure with the eye tracker"). Source: docs/GUI.md, docs/LESSONS.md rows 11 and 18.

### 6.2 Eye calibration
- Run the app (eye build, desk/VR stopped, worn): `pimax-eyecal` (keys: space/a next, r redo, y save, p pursuit).
- Refit an existing run (pending for run `eyecal-20261014-103635`, which reaches 2.38 deg against 3.08 deg saved): `pimax-eyecal --refit ~/.local/share/omarchyvr/eye-calibration/eyecal-20261014-103635.json` (prints the table), then `--write ~/.config/omarchyvr/eye-calibration.toml` to save. The old file is kept as `.bak-<utc>`. Needs a Monado relink (v2 format) and setcap first.
- Device-side calibration (blob 0x456 and the other 0x3f2/0x407/0x42e/0x3fc ops) stays off until the user approves it. Sending the blob once made the chip stop.
- GUI: yes. Tracking > Eyes (developer mode) > Refit the newest run prints the table, and Save the refit writes the calibration after a confirmation. Save stays off until the eye build reads version 2 files (checked in the binary). The eye calibration file is shown in Setup (docs/GUI.md row 18).

### 6.3 Controller gyro (imufit)
- After a 6DoF desk session with controllers, the logs are in `/tmp/omarchyvr-session/ctrl-imu-{left,right}.csv`.
- Fit and save: `~/pimax-re/rust/target/release/pimax-ctrltrack imufit /tmp/omarchyvr-session/ctrl-imu-*.csv --save` (writes `~/.config/omarchyvr/ctrl-imu.json`). Without `--save` it only prints.
- The fit also saves every 30 s and at shutdown.
- Source: analysis/controller-6dof.md.
- GUI: yes. Tracking > Tests (developer mode) > Fit controller gyros and Save the fit (after a confirmation), when the last session left logs.

### 6.4 Room map (persistent, no tag)
- Script: `tools/room-test.sh`. Default: build the map if missing, restart desk (`--6dof --ctrl-track --preset work --anchor`), relocalize and apply.
  - `tools/room-test.sh --rebuild`: map the room again first.
  - `tools/room-test.sh --reloc`: only relocalize and apply in the running session.
  - Override the desk args with `DESK_ARGS=...`.
- Manual equivalent: `omarchyvr-roommap build --source broker --seconds 60 --name home` (walk the room), then `omarchyvr-roommap relocalize --source broker --name home --apply` after each 6DoF start; look around for 3-7 s.
- Floor (new, commit 477eab0): `omarchyvr-roommap floor detect|drift|touch|set|show|follow|preview`. Not checked in use.
- Map directory: `~/.local/share/omarchyvr/roommap/<name>/`.
- Root: no. GUI: yes ("Map my room", "Recognise room"). Source: tools/room-test.sh, docs/INVENTORY.md 2.15, docs/GUI.md.

### 6.5 AprilTag anchor
- Print the tag: `omarchyvr-anchor tag` (writes SVG and PDF).
- Save reference (headset held still, tag in view 4 s, windows placed): `omarchyvr-anchor calibrate`.
- Apply: `omarchyvr-anchor apply` (yaw and translation; `--full` adds roll and pitch; `--dry-run` writes nothing).
- Clear: `omarchyvr-anchor clear`.
- Needs a desk session with 6DoF, the broker and `--anchor`. Only one of roommap and anchor writes the offset per session.
- Root: no. GUI: yes (Room tab). Source: docs/INVENTORY.md 2.16, docs/GUI.md.

## 7. Recovery

### 7.1 Xid preflight and reboot
- Check: `journalctl -k -b | grep -E 'Xid|Idling display engine'` (must be empty before a session).
- If it shows Xid or "Idling display engine": reboot before the next start. `omarchyvr vr stop` exits 2 for this.
- `omarchyvr vr reset-headset [--mode exit|dp] [--wait SECS]` ("Restart headset") resets the headset side only (it also brings back a dropped camera, section 7.4). It does not clear a stuck NVIDIA display, so that needs a reboot.
- Root: the check needs no root; the reboot does. GUI: yes (kernel log is classified line by line).

### 7.2 Stopping by PID only
Never SIGKILL Monado. Use `omarchyvr vr stop --grace 30` or `omarchyvr desk stop --grace 30`. Kill by PID only, never by name pattern (docs/ARCHITECTURE.md).

### 7.3 Logs
- Monado: `/tmp/omarchyvr-session/monado.log`.
- Desk session: the session dir (passthrough.log, wayvr.log, wayvr.stdout, voice*.log).
- Crash reports: `~/.local/state/omarchyvr/crashes/<time>-<kind>/report.txt` (minidump.dmp when ticked).
- Coredumps: `coredumpctl list` (diagnose-crash skill).
- Launcher and auto mode: `journalctl --user -u omarchyvr-autod -f`.
- GUI: yes (Logs, crash reports, problem report).

### 7.4 Headset power cycle ("Restart headset")
`omarchyvr vr reset-headset --mode exit` (default, `--wait SECS`, default 150) or `--mode dp`. Both restart `omarchyvr-keepalived` and refuse while monado-service runs. The GUI button is "Restart headset".
- Camera gone (USB3 side dropped, `usb 2-4: USB disconnect`, "camera 34a4:0014 not found under /sys/class/video4linux")? No physical unplug is needed. Measured 2026-10-15: the HID chip stayed, the camera re-enumerated by itself 09:26:31, about 90 s after the reset started (and after keepalived restarted). The tool tracks a camera that was absent at the start, checks the `/sys/class/video4linux` node and reports "camera back after N s" (verdict).
- `omarchyvr desk start` and `omarchyvr vr start --6dof` do this automatically when the camera is missing but the HID is present (no session running, up to 150 s, progress lines), then continue. `--no-auto-recover` skips it. Preflight shows `[warn] camera ... camera missing: will auto-recover`.
- If the exit reset does not bring the camera back, the tool retries once by itself with `--mode dp`, then says to run Restart headset again or reboot. Power-cycling the link box is only the very last resort.
- Restart without losing flags: `omarchyvr desk restart` / `omarchyvr vr restart` stop with grace 30 and start again with the SAME arguments as the last start (recorded in `<session dir>/start-argv.txt` on every start; the camera is recovered automatically in between).
- Session watchdog: `desk start` / `vr start` spawn a detached `pimax <desk|vr> watchdog` (log `<session dir>/watchdog.log`, `--no-watchdog` or `PIMAX_NO_WATCHDOG=1` to skip; not with `--wait`/`--dry-run`). If monado-service dies, or the HID 34a4:0044 / the camera 34a4:0014 (when passthrough or 6DoF needs it) is gone for 6 s, it notifies "Headset reconnecting...", does stop, recover, start with the recorded flags, at most 3 times per 10 minutes, then a critical notification. A user `stop` is never fought (marker `user-stop`, `stop.lock`; a newer start replaces the watchdog via `watchdog.pid`).
- Root: no. GUI: button "Restart headset" (runs the CLI).

## 8. Pairing and auto mode

### 8.1 Controller pairing
```
omarchyvr-autod pair left        # or: omarchyvr-autod pair right
omarchyvr auto pair left         # through the running daemon
```
- Press and hold the controller's pairing buttons (see the Pimax manual). Waits up to 35 s; prints `waiting` every 5 s, then `paired`, `timeout` or `failed`. Exactly one StartPair is sent per click; never retried.
- Approved by the user 2026-10-14 (docs/LESSONS.md row 11). Works without the daemon.
- Root: no. GUI: yes (one StartPair per click, 35 s progress).

### 8.2 Auto mode control
```
omarchyvr auto status          # one JSON line
omarchyvr auto events          # stream
omarchyvr auto pause | resume  # no starts or stops
omarchyvr auto wake            # "I'm wearing it"
omarchyvr auto sleep           # stop now (own, non-game session)
omarchyvr auto set-mode desk|game --runtime monado|steamvr --preset cinema
```
- Daemon: `omarchyvr-autod run` (dry run unless `~/.config/omarchyvr/auto.toml` says `dry_run = false`); config check: `omarchyvr-autod config`.
- Live checklist: analysis/auto-mode-live-checklist.md.
- `omarchyvr-autod visor-probe` is LIVE TEST ONLY (sends the Windows 0x2c read). Not a daily step.
- Root: no. GUI: yes. Developer > Auto mode sets Desk or Game, the runtime (Monado or SteamVR) and the preset. Setup > Install installs the unit.

## GUI coverage gaps

Checked against the app's code (omarchyvr-gui-core, omarchyvr-app) on 2026-10-15. Root steps are run through pkexec after the user confirms them, or copied when they are not a documented fix shape.

| # | Manual step | Root | GUI today | Still open |
|---|---|---|---|---|
| 1 | EDID override and modeset config (1.1), non-desktop EDID | yes | partly: which EDID is installed; the doctor's connector lines, copy only | none: copy only on purpose (machine-specific lines) |
| 2 | Hyprland DP-1 monitor line (1.2) | no | partly: read-only check of `~/.config/hypr`, the line to add or remove | none: the app never edits Hyprland's config |
| 3 | udev rule install (1.3) | yes | yes: Install, Checks and the onboarding step run the documented lines through pkexec (reload, trigger) | none |
| 4 | Keepalive drop-in and enable (1.4) | no | yes: Install > Keepalive unit (link, Rust drop-in, daemon-reload, enable; only while idle) | none |
| 5 | OpenVR runtime switch xrizer/SteamVR (1.6) | no | yes: Developer > VR runtime, Tools > Steam runtime | none |
| 6 | NVIDIA pacman hooks and patches (1.7) | yes | yes: hooks and patch state; install and reapply through pkexec | none |
| 7 | Packages: rust, steam, Proton (1.8) | yes | partly: cargo, Steam and Proton are checked; pacman lines copy only | none: the app runs no package installs |
| 8 | Eye tracker power-on (1.9) | no | yes: Power on eye module (Tools, Tracking > Eyes) | auto-enable after a power cycle (daemon side, planned) |
| 9 | setcap after every Monado relink (2) | yes | yes: getcap per build every 10 s, the setcap dialog right after the app's own rebuild, pkexec | none |
| 10 | Build and install tools (3.1-3.3) | no | yes: Install > Rust tools, pimax launcher, this app (build, install to `~/.local/bin`) | none (`omarchyvr-autod` runs from the workspace build) |
| 11 | Auto-mode unit install (3.5) | no | yes: Install > Auto mode daemon; Developer > Auto mode: Start, Start at login | none |
| 12 | Steam launch options (5) | no | partly: the line with Copy; `PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1 %command%` once the launcher set `active_runtime.json` | writing Steam's `localconfig.vdf`: not planned (Steam rewrites it while it runs) |
| 13 | Eye calibration refit and save (6.2) | no | yes: Tracking > Eyes: Refit the newest run, Save the refit (confirmed, old file kept) | none |
| 14 | Controller gyro imufit --save (6.3) | no | yes: Tracking > Tests: Fit controller gyros, Save the fit (confirmed) | none |
| 15 | Refit path for Monado relink v2 format (6.2) | yes | yes: Save stays off until the eye build reads version 2 files, with the rebuild and setcap steps | none |
| 16 | Auto-mode `set-mode --runtime steamvr` (8.2) | no | yes: Developer > Auto mode: Desk or Game, runtime, preset | none |

## Sources
- memory/pimax-linux-re.md; README.md; STATUS.md; HISTORY.md; NEXT-STEPS.md
- linux/DISPLAY.md; linux/nvidia-pimax-patch.hook; linux/nvidia-patch-reapply.sh; linux/70-pimax.rules; linux/omarchyvr-keepalived.service
- docs/AUTO-MODE.md; docs/GUI.md; docs/LESSONS.md; docs/INVENTORY.md; docs/ARCHITECTURE.md
- analysis/steamvr-native-linux.md; analysis/steamvr-games.md; analysis/controller-6dof.md; analysis/eye-precision.md; analysis/auto-mode-live-checklist.md; analysis/room-map.md
- tools/room-test.sh; packaging/README.md; packaging/systemd/omarchyvr-autod.service
- `pimax --help`, `omarchyvr desk help`, `omarchyvr vr help`, `omarchyvr-autod --help`, `omarchyvr vr steam-options`
- rust/omarchyvr-tools/src/vr/config.rs; session transcripts (grep only)
