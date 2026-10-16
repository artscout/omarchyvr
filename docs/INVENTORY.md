# Pimax Dream Air on Linux: inventory

Purpose: one file for designing a GUI installer and launcher. Facts come from the repo (README.md, STATUS.md, HISTORY.md, NEXT-STEPS.md, linux/, analysis/, rust/*/Cargo.toml and CLI source), the forks in `~/src`, and a read-only look at what is installed on this machine (2026-10-14, after commit 5d0e4d9). Nothing here starts the headset, Monado or a device tool.

Conventions: **root** = needs sudo or root. **user** = no root. "untested live" = built and tested offline or in a harness only. Commit hashes are from this repo unless noted with a fork name.

Contents: 1 Components | 2 Commands | 3 Install and dependencies | 4 Problems and fixes history | 5 Open issues | 6 Safety rules

---

## 1. Components

Status key: **working** = used live and passing; **experimental** = runs, limited live use; **untested live** = not yet run on the headset; **offline** = analysis tool, no device; **test-only** = not shipped.

### 1.1 Rust binaries and libraries (`~/pimax-re/rust`, workspace; release build in `rust/target/release/`)

| name | kind | path | what it does | status | command to run it |
|---|---|---|---|---|---|
| pimax (omarchyvr-tools) | Rust binary | `rust/omarchyvr-tools/`; installed copy `~/.local/bin/omarchyvr` | Launcher and CLI: keepalived, probe, imu, controllers, calib, video-on, logtail, info, vr, desk, mount-shared | working | `omarchyvr vr start` |
| omarchyvr-keepalived | user systemd service (Rust via drop-in) | `~/.config/systemd/user/omarchyvr-keepalived.service` -> `linux/omarchyvr-keepalived.service`; drop-in `~/.config/systemd/user/omarchyvr-keepalived.service.d/rust.conf` | Wake + keepalive every 213 ms, drains IF1-IF3 | working (always on) | `systemctl --user status omarchyvr-keepalived` |
| omarchyvr-doctor | Rust binary | `rust/omarchyvr-doctor/` | Read-only health checks: USB/udev, keepalive, NVIDIA patches (ids viewport, flip-fix, bounded-wait), EDID connector, lease, setcap, kernel log. Fix plan only, never runs it | working (13/13 OK per HISTORY) | `omarchyvr-doctor` |
| omarchyvr-app ("Pimax" app) | Rust egui GUI + core lib | `rust/omarchyvr-app/` (UI), `rust/omarchyvr-gui-core/` (state, LESSONS preflight, checklist, tools, safety); desktop entry `linux/omarchyvr.desktop` | Desk mode start/stop with the LESSONS preflight and live status, setup checklist with fixes (pkexec for root), tools, logs and history, first-run wizard, profiles. See [GUI.md](GUI.md) | built and tested offline; desk start/stop from the app not yet exercised live | `cd ~/pimax-re/rust && cargo run --release -p omarchyvr-app` |
| pimax-eye | Rust binary + lib | `rust/pimax-eye/` | Tobii EyeChip (2104:0561) TTP, replay, live tracker for Monado, watch view, IPD measure and motor, power-on, pcap decode | working for streaming (119 Hz, both eyes); accuracy not solved (8.8 deg raw) | `pimax-eye replay --skip-cal --skip-vendor-write` |
| pimax-eyecal | Rust OpenXR/Vulkan app | `rust/pimax-eyecal/` | Eye calibration and validation app, pursuit, live cursor, runtime check | experimental, untested live (per analysis/eye-precision.md not yet run worn) | `analysis/tools/eye_calibrate.sh` |
| pimax-ipd | Rust binary + lib | `rust/pimax-ipd/` | Guided IPD test shown in the headset (omarchyvr-hud): countdown, eye-chip measurement, measured vs lens IPD. `--apply` moves the lenses (the approved IPD command) and refuses a measurement that looks wrong. `manual` picks the IPD by keys; `--simulate` writes the screens as PNG without a headset | built and tested offline (fake devices, `--simulate`); no live run recorded in the docs | `pimax-ipd --simulate` (no headset); `pimax-ipd` measures only |
| omarchyvr-hud | Rust lib (OpenXR/Vulkan) | `rust/omarchyvr-hud/` | In-headset text panel with a progress bar: an OpenXR quad layer, head-locked or placed at start, optional `XR_EXTX_overlay` session, CPU-rendered with the built-in 5x7 font. Also renders to PNG without a headset | tested offline (rendering to PNG); used by pimax-ipd; no live run recorded in the docs | (library) |
| pimax-cam | Rust binary + C ABI | `rust/pimax-cam/` (`include/pimax_cam.h`) | Camera capture (UVC 34a4:0014), tile split, metadata, KB4 calibration, recording, broker, SLAM pose output, AE controller | working (60 fps, 0 lost). AE: never sends unless `PIMAX_CAM_AE=1` and approved | `flock -w 3600 /tmp/omarchyvr-headset.lock pimax-cam broker` |
| omarchyvr-passthrough | Rust OpenXR client | `rust/omarchyvr-passthrough/` (built to `~/src/target-passthrough/release/` or `~/src/target-desk*/`) | Camera passthrough, KB4 fisheye to per-eye maps, timewarp does IMU rotation | working (25 fps, about 41 ms) | `~/src/target-passthrough/release/omarchyvr-passthrough run` |
| omarchyvr-perf | Rust binary + lib | `rust/omarchyvr-perf/` | Performance readout on the right wrist: reads the stats file Monado writes (`PIMAX_PERF_FILE`) and shows HMD fps, app fps, missed frames and app CPU/GPU ms on a WayVR panel over `wayvrctl batch`, coloured green, yellow or red. Pimax app: Display > Performance (developer mode) | experimental; on by default in `omarchyvr desk start` (`--no-perf` turns it off); game mode `omarchyvr vr start --perf` not yet tried with a real game (docs/WAYVR-CONTROLS.md) | `omarchyvr-perf run` (the launcher starts it) |
| omarchyvr-slam | Rust offline tools | `rust/omarchyvr-slam/` | Basalt export, EuRoC check, IMU noise, camera timing, ATE/RPE, loop drift, AE dry run | offline (6DoF live not yet done) | `omarchyvr-slam export` |
| omarchyvr-room | Rust prototype | `rust/omarchyvr-room/` | Room map, relocalisation, anchors | experimental, offline | `omarchyvr-room demo` |
| omarchyvr-roommap | Rust binary + lib | `rust/omarchyvr-roommap/` | Persistent room map without a tag: build a map of the room from a 6DoF session (ORB stereo keyframes, loop closure), recognise it in a later session and write the Monado world offset (the `PIMAX_ANCHOR` hook, `--anchor` in the launcher), so the world origin and the WayVR windows come back to the same real place. Pimax app: Room page | offline, untested live: validated on 6 recordings (no false positives, same-place error 15-81 mm median over 7 session pairs); live test pending (analysis/room-map.md sections 3 and 5) | `omarchyvr-roommap build --source broker --seconds 90 --name home` |
| omarchyvr-anchor | Rust binary + lib | `rust/omarchyvr-anchor/`; printable tag in `analysis/anchor/` | Room anchor with a printed AprilTag (tag36h11) on the desk: the headset cameras detect it and, with the head pose Monado reports (`PIMAX_ANCHOR` hook, `--anchor` in the launcher), the tool writes the Monado world offset that brings the VR windows back to the same real spot. Pimax app: Room page | built; tests use synthetic tags (analysis/anchor/README.md); no live run recorded in the docs | `omarchyvr-anchor detect` |
| pimax-selfcal | Rust offline tool | `rust/pimax-selfcal/` | Camera self-calibration, overlay `~/.config/omarchyvr/cam-calib-overlay.json` with rollback | offline (merged; 1-minute lit recording still owed) | `pimax-selfcal selftest` |
| omarchyvr-voice | Rust binary | `rust/omarchyvr-voice/` (`target-voice/release/`) | Local STT: PipeWire mic, Silero/energy VAD, Parakeet v3 via sherpa-onnx, JSON-lines socket, intents | experimental, untested in headset (mic sends zeros while the panel is off) | `omarchyvr-voice listen` |
| omarchyvr-voice-hud | Rust OpenXR overlay | `rust/omarchyvr-voicehud/` (`target-voicehud/release/omarchyvr-voice-hud`) | Head-locked caption quad from the omarchyvr-voice socket | experimental, untested live | `omarchyvr-voice-hud` |
| pimax-hands | Rust lib + C ABI | `rust/pimax-hands/` | Controller-driven hand poses, rest placement | working (xr-handtest PASS) | linked by Monado patch 0014 |
| pimax-lens | Rust binary + staticlib | `rust/pimax-lens/` (`include/pimax_lens_rs.h`) | Lens model used by Monado, lens.conf, fit-pvr, LUT | working (default lens.conf = PVR fit) | `pimax-lens show` |
| pimax-lenstest | Rust OpenXR/Vulkan client | `rust/pimax-lenstest/` | Test patterns, gaze accuracy pattern, captures | working | `pimax-lenstest list` (drive with `analysis/tools/run_lens_test.sh`) |
| pimax-capi | cdylib + staticlib | `rust/pimax-capi/` (`libpimax_capi.so`, `.a`) | C ABI of libpimax (`include/pimax.h`) | working (Monado `PIMAX_USE_RUST_LIBPIMAX=ON`) | `cargo build --release` |
| pimax-proto | no_std lib | `rust/pimax-proto/` | Framing and decoders | working | (library) |
| pimax (lib) | Rust lib | `rust/pimax/` | hidraw I/O, session thread | working | (library) |
| omarchyvr-config | Rust lib | `rust/omarchyvr-config/` | Per-serial profiles, atomic writes, backups, restore, diff, legacy import | working | used by omarchyvr-app |
| xr-handtest, xr-gaze, xr-pinchtest | Rust OpenXR test clients | `rust/omarchyvr-xrtest/` | Hand tracking check, gaze at 10 Hz, pinch check | working | `xr-handtest --source any` |
| omarchyvr-desktest | Rust offline analysis | `rust/omarchyvr-desktest/` | Monado metrics, compositor dump analysis | offline | `omarchyvr-desktest metrics FILE` |
| omarchyvr-voicebench | Rust CPU latency harness | `rust/omarchyvr-voicebench/` | Parakeet/Kokoro latency via sherpa-onnx | offline | `omarchyvr-voicebench bench` |
| pimax-difftest, pimax-lens-difftest | test-only | `rust/pimax-difftest/`, `rust/pimax-lens-difftest/` | Differential tests against the original C libpimax and C lens model | test-only | `cargo test --workspace` |
| rust/systemd/omarchyvr-keepalived-rust.service | systemd unit template | `rust/systemd/` | Rust keepalive unit, NOT installed (superseded by the drop-in) | superseded | none |

### 1.2 Python tools, scripts and C test clients (repo, not installed)

| name | kind | path | what it does | status | command |
|---|---|---|---|---|---|
| pimax_keepalived.py | Python | `linux/` | Original keepalive (its unit file still points here; the drop-in overrides ExecStart) | superseded | `python3 linux/pimax_keepalived.py` |
| pimax_probe.py, pimax_imu_orient.py, pimax_controllers.py, pimax_calib.py, pimax_logtail.py, pimax_video_on.py, pimax_keepalive_test.py | Python | `linux/` | Original probes; each maps to a `omarchyvr` subcommand (see README cheat sheet) | superseded, kept as references | `python3 linux/pimax_probe.py` |
| make_hmd_edid.py | Python | `linux/` | Builds DisplayID 2.0 HMD EDIDs (dead end, non-desktop solved by VSDB) | reference | `python3 linux/make_hmd_edid.py` |
| pimax_display_on.sh | bash (sudo for debugfs/sysfs) | `linux/` | Original MVP panel-on at 7104x1840 | superseded | do not use |
| wait_and_probe.sh | bash (sudo) | `linux/` | Wait for 34a4:0044, then probe | superseded | `sudo linux/wait_and_probe.sh 25` (root) |
| nvidia-patch-reapply.sh | bash | `linux/` | Applies the three NVIDIA patches to `/usr/src/nvidia-<ver>`, idempotent; `--check`, `--hook`, `--rebuild`, `--verify`, `--version`, `--src-root`, `--patch` | working; used by both pacman hooks | `linux/nvidia-patch-reapply.sh --check` |
| test_nvidia_patch_reapply.sh | bash test | `linux/` | Tests the reapply script on scratch copies (no root) | test-only | `linux/test_nvidia_patch_reapply.sh` |
| analysis/tools/*.sh (20 scripts) | bash glue | `analysis/tools/` | Session launchers, see 2.20 | experimental to working | see 2.20 |
| analysis/tools/*.c, *.cpp, prebuilt binaries | C/C++ test clients | `analysis/tools/` (xr_room, xr_grid, xr_eyetest, xr_eyetest_srgb, vk_display_test, kms_lease_test, posecli, ovr_smoke, ctrlcli) | Vulkan/KMS/OpenXR tests, lease probes | test tools | built by hand, see headers |
| linux/make_hmd_edid.py output | data | `linux/pimax-dp-edid-*.bin` | EDID variants (native, msvsdb, hmd, 1080p, tall, wide, dual) | see 3.5 | none |
| linux/pimax_calib.py | Python | `linux/` | Reads the factory calibration XML (command 0x0a, 84 reads) | reference | `omarchyvr calib` |
| analysis/tools/run_*.sh (headset) | bash | `analysis/tools/` | Need the headset and the lock | experimental | see 2.20 |
| analysis/tools/run_vr_color_test.sh | bash | `analysis/tools/` | Colour pipeline check (opens the display) | experimental | do not run unattended |

### 1.3 Systemd units, udev, pacman hooks, kernel-side patches

| name | kind | path (repo) | installed path | what it does | status | command |
|---|---|---|---|---|---|---|
| omarchyvr-keepalived.service | systemd user unit | `linux/omarchyvr-keepalived.service` | `~/.config/systemd/user/` (symlink) + drop-in `rust.conf` | keepalive daemon, Restart=always | working | `systemctl --user restart omarchyvr-keepalived` (do not stop by hand) |
| 70-pimax.rules | udev rule | `linux/70-pimax.rules` | `/etc/udev/rules.d/70-pimax.rules` (present) | hidraw and USB 34a4 and 2104:0561 get MODE 0660, uaccess | working | `sudo udevadm control --reload && sudo udevadm trigger` (root) |
| 65-nvidia-pimax-patch.hook | pacman hook (PostTransaction, Install/Upgrade of nvidia-open-dkms) | `linux/nvidia-pimax-patch.hook` | `/etc/pacman.d/hooks/65-nvidia-pimax-patch.hook` (present) | runs `/usr/local/lib/omarchyvr/nvidia-patch-reapply.sh --hook` before dkms builds | working (installed; untested by a real upgrade in this review) | root-installed |
| 75-nvidia-pimax-verify.hook | pacman hook (after dkms) | `linux/nvidia-pimax-verify.hook` | `/etc/pacman.d/hooks/75-nvidia-pimax-verify.hook` (present) | `--verify`: modinfo nvidia-modeset must list merged_head_sync | working | root-installed |
| /usr/local/lib/omarchyvr/ | root-owned copies | `linux/nvidia-patch-reapply.sh`, three `.patch` files | present | pacman runs the hooks as root, so the hook never points at a home directory | working | root |
| nvidia-2h1or-viewport.patch | kernel module (out-of-tree NVIDIA) patch | `linux/` | applied to `/usr/src/nvidia-610.57.04` (backup `.orig`) | fixes nvEvoSetViewPorts x=0 on every merged head (upstream PR #1262) | working (verified 0f3d233) | via reapply script |
| nvidia-2h1or-flip-fix.patch | kernel module patch | `linux/` | same tree; adds `merged_head_sync` param | sync objects on the primary merged head; fixes the freeze | working; root cause "most likely", not proven | via reapply script |
| nvidia-evo-makeroom-bounded-wait.patch | kernel module patch | `linux/` | same tree | bounded wait in EVO make-room (stop-gap) | working | via reapply script |
| nvidia-modeset parameter | module option | `/etc/modprobe.d/nvidia-pimax.conf` | present: `options nvidia-modeset config_file=/etc/pimax/nvidia-modeset.conf` | loads the boot EDID override | working | root |
| DKMS | build system | `/usr/src/nvidia-610.57.04` | dkms.conf present | rebuilds the patched module | working | `sudo dkms build/install` (root) |

### 1.4 Monado and WayVR forks (outside this repo)

| name | kind | path | what it does | status | command |
|---|---|---|---|---|---|
| Monado `omarchyvr` | Monado fork (C) | `~/src/monado`, branch `omarchyvr`, build `~/src/monado/build/src/xrt/targets/service/monado-service` (cap_sys_nice set) | Pimax driver (3DoF, lens, controllers, cameras, hands, canting, flipsafe, gyro timeout). Patches 0001-0019 in `monado/` | working (direct mode, 72 Hz) | `omarchyvr vr start` |
| Monado `eye` | Monado fork | `~/src/monado-eye`, branch `eye`, build `~/src/monado-eye/build/...monado-service` (cap_sys_nice set) | pimax + eye gaze (XR_EXT_eye_gaze_interaction) behind `PIMAX_EYE=1` | working (gaze streams) | `omarchyvr desk start --eye` |
| Monado `slam` | Monado fork | `~/src/monado-slam`, branch `slam`, build `~/src/monado-slam/build/...monado-service` (NO cap_sys_nice) | 6DoF via Basalt behind `PIMAX_SLAM=1`, EuRoC recording | untested live (offline only) | `PIMAX_SLAM=1 PIMAX_MONADO_BUILD=~/src/monado-slam/build omarchyvr vr start` (after setcap) |
| Monado worktrees | branches | `~/src/monado-flipsafe`, `-hangfix`, `-lens-base`, `-passthrough`, `-cant`, `-hands`, `-cam`, `-lens`, `-polish` | older branch worktrees; `omarchyvr` merged them | reference | none |
| WayVR `eye-gaze` | WayVR fork (Rust) | `~/src/wayvr`, branch `eye-gaze` (checked out, WayVR 26.8.0 base), built to `~/src/target-desk/release/wayvr` and `wayvrctl` | overlay desktop with gaze pointer, hands from XR_EXT_hand_tracking, pinch click | experimental (hands and gaze not confirmed visible in the headset) | started by `omarchyvr desk start` |
| xrizer | OpenVR-to-OpenXR layer | `~/src/xrizer` | SteamVR games (64-bit) | working (Dungeons of Eternity) | `vr_runtime_switch.sh xrizer` |
| Basalt | VIO library | `~/src/basalt`, installed in `~/.local` | 6DoF back end for the slam branch | offline | (via Monado slam build) |
| Monado tree mirror | patches | `monado/*.patch` in this repo | 19 numbered patches of the pimax branch | reference | `git am` onto upstream Monado |

Fork branch and commit state (checked 2026-10-14):

| repo | branch checked out | last 5 commits |
|---|---|---|
| `~/src/monado` | `main` (branches: pimax*, slam, eye, cant, flipsafe, hangfix, polish, cam, lens, hands, lens-base) | `omarchyvr` HEAD 4ec0348 Merge branch 'cant'; 7c3831a pimax: display canting; 02889c9 comp: survive a stuck NVIDIA flip (flipsafe); e64d487 comp_renderer: drop stuck state; be4d119 lazy controller-hands feed and bounded GPU waits |
| `~/src/monado-eye` | `eye` | 43516a5 pimax: controller placement from pimax-hands; 99596a9 pimax: periodic eye gaze stats; 1025754 pimax: eye gaze (XR_EXT_eye_gaze_interaction) from the Rust EyeChip session; 4ec0348 Merge branch 'cant'; 7c3831a pimax: display canting |
| `~/src/monado-slam` | `slam` | d3138d4 pimax: gate the IMU fed to the SLAM tracker so a camera gap cannot deadlock Basalt; 16701d4 tracker/recorder frame time at mid-exposure; 9dfbadc 6DoF head pose from Rust; cb9ff16 6DoF via Monado SLAM behind PIMAX_SLAM=1; 4ec0348 Merge branch 'cant' |
| `~/src/wayvr` | `eye-gaze` | 36a2e27 Hands: opaque amber 6 mm strokes; 7d0ab4a Gaze: gaze_over_controllers; 88db4e0 Draw hands from XR_EXT_hand_tracking; a8a2469 Add eye-gaze pointer; cb3aa42 Release 26.8.0 |

### 1.5 Models, configs, state directories

| name | kind | path | what it is | status |
|---|---|---|---|---|
| ~/models/parakeet-tdt-0.6b-v3-int8 | STT model (default) | `~/models/` | Parakeet TDT 0.6B v3, English + Spanish | present |
| ~/models/parakeet-tdt-0.6b-v2-int8 | STT model | `~/models/` | older | present |
| ~/models/kokoro-multi-lang-v1_0 | TTS model (voicebench) | `~/models/` | Kokoro-82M | present |
| ~/models/whisper/ggml-*.bin, ~/models/faster-whisper, ~/models/streaming | optional STT | `~/models/` | whisper engine needs a build with the whisper feature | present |
| ~/models/vad/silero_vad.onnx, ten-vad.onnx | VAD | `~/models/vad/` | default VAD is silero when silero_vad.onnx exists | present |
| sherpa-onnx C API `.so` | runtime library | found under `~/src/venv-voice` site-packages; passed with `--lib` | used by omarchyvr-voice and voicebench (dlopen) | present |
| ~/.config/omarchyvr/lens.conf | per-user lens config | `~/.config/omarchyvr/` | default lens (PVR fit, cant_deg 2.0) read by Monado, lenstest | working |
| ~/.config/omarchyvr/lens.conf.bak-* | backups | `~/.config/omarchyvr/` | older lens variants | reference |
| ~/.config/omarchyvr/lens-ipd/ | IPD and centre variants | `~/.config/omarchyvr/lens-ipd/` | c0.0071.conf ... c0.0521.conf, pvr.conf, pvr-lut.conf | reference |
| ~/.config/omarchyvr/eye-calibration.toml | eye calibration (pimax-eyecal save) | not present yet | written by pimax-eyecal `y` (old file kept as .bak-<utc>) | not created |
| ~/.config/omarchyvr/eye-correction.toml | host-side gaze correction | not present | PIMAX_EYE_CORRECTION | not created |
| ~/.config/omarchyvr/ipd.env | last applied IPD | not present | written on quit of `pimax-eye ipd --watch` after an apply | not created |
| ~/.config/omarchyvr/cam-calib-overlay.json | selfcal overlay | not present | installed only by `pimax-selfcal estimate --write` | not created |
| ~/.config/omarchyvr/slam/, slam-imu.json | 6DoF export and IMU noise | not present | `omarchyvr-slam export` and `imu-noise --write` | not created |
| ~/.cache/omarchyvr/gyro-bias | cached gyro bias | `~/.cache/omarchyvr/` | 3 s timeout fallback in Monado patch 0018 | present |
| ~/.local/share/omarchyvr/eye-accuracy, eye-calibration, slam-recordings | outputs | `~/.local/share/omarchyvr/` | results JSON, raw CSV, EuRoC recordings | present |
| WayVR home `conf.d/10-am.yaml` | WayVR config (path from `--wayvr-home`) | written by `omarchyvr desk start` if absent | `capture_method: screencopy` (avoids dmabuf Xid at exit) | present |
| /etc/pimax/dp-edid.bin | boot EDID | `/etc/pimax/` (root) | identical to `linux/pimax-dp-edid-msvsdb.bin` (native modes, 8 bpc, HMD VSDB for non-desktop) | installed |
| /etc/pimax/nvidia-modeset.conf | modeset override | `/etc/pimax/` (root) | `override.[0000:01:00.0].DP-0=/etc/pimax/dp-edid.bin` | installed |
| /etc/mkinitcpio.conf.d/pimax.conf | initramfs | root | adds the two files to the initramfs | installed |
| /tmp/omarchyvr-headset.lock | headset lock | `/tmp` | flock for all device tools (the only claimant rule) | runtime |
| /tmp/omarchyvr-session/ | launcher session dir | `/tmp` (PIMAX_VR_SESSION_DIR) | monado.log, passthrough.log, wayvr.log, voice logs | runtime |
| $XDG_RUNTIME_DIR/omarchyvr-voice.sock | voice socket | runtime | JSON lines to the HUD | runtime |

---

## 2. User-facing commands

Flags are taken from the usage text and argument parsing in the source. Device commands must run under the lock: `flock -w 3600 /tmp/omarchyvr-headset.lock <cmd>`.

### 2.1 `omarchyvr` (rust/omarchyvr-tools, installed at `~/.local/bin/omarchyvr`)

| subcommand | flags (from source) | what | headset? |
|---|---|---|---|
| `keepalived` | `[--no-wake] [--rate-file P]` | daemon: wake + 213 ms keepalive, drain IF1-IF3, auto-reconnect | yes (USB) |
| `probe` | `[seconds] [--passive]` | wake/keepalive unless passive, identity reads, IMU and log summary | yes |
| `imu` | `[--json] [--rate HZ] [--no-calib] [--calib F] [--auto-bias S] [--still N] [--seconds T] [--kp K] [--ki K] [--axis-mode 0\|1]` | Mahony orientation from IF2, read-only; axis mode 1 default | yes (read) |
| `controllers` | `[seconds] [--raw]` | attach and decoded controller reports | yes |
| `calib` | `[out.xml] [--blob F] [--save-blob F]` | read the factory calibration (84 reads), inflate, diff; `--blob` decodes offline | yes unless `--blob` |
| `video-on` | `[fps=72] [seconds=40]` | switches the panel on (changes headset state) and tails logs | yes, changes state |
| `logtail` | `[seconds=30] [regex]` | passive firmware and bridge log tail | yes (read) |
| `info` | `[--controllers]` | serial, version, interfaces | yes (read) |
| `vr start` | `[--fps 72\|90] [--dump DIR [--dump-frames 100,300]] [--wait] [--dry-run] [--force] [--debug] [--no-notify] [--monado-build DIR] [--connector NAME] [--mode N] [--timeout SECS]` plus colour experiments `[--gamma G] [--gain K] [--target-format ...]` (only with the srgb patch) | preflight, then monado-service detached; returns when the gyro calibration is done. Preflight covers: headset on USB, keepalived active, DP-1 connected, cap_sys_nice, and no Xid or "Idling display engine" in this boot's kernel log | yes |
| `vr stop` | `[--grace SECS] [--all]` | SIGINT to the started service, SIGKILL after 5 s (15 s if a client is connected), then kernel-log check. `--all` also stops your other monado-service processes | yes |
| `vr status` | `[--log FILE] [--notify]` | service, headset, display, kernel state, log summary; exit 3 if not running | read |
| `vr reset-headset` | `[--mode sys\|exit\|dp] [--session-dir DIR]` | `sys` (default): the headset restart command `58 04 28 00 00`, then the exit mode if the headset ignores it. `exit`: replays the Pimax Play exit frames (headset power cycle). `dp`: bridge power cycle via keepalive stop. Refuses while monado-service runs. Does NOT clear a stuck NVIDIA display | yes |
| `vr steam-options` | none | prints the Steam launch options line | no |
| `vr dump-stats` | `FILE.raw... [--step N] [--probe X,Y]... [--ppm OUT]` | colour stats of compositor dumps | no |
| `vr dump-diff` | `A.raw B.raw` | pixel diff of two dumps | no |
| `desk start` | `[--no-passthrough] [--eye] [--voice] [--settle SECS] [--passthrough-bin P] [--wayvr-bin P] [--wayvr-home DIR] [--voice-bin P] [--voice-hud-bin P]` + all `vr start` options | monado-service, then omarchyvr-passthrough run, then WayVR overlay. `--eye` uses `~/src/monado-eye/build` and PIMAX_EYE=1. `--voice` = live captions on a WayVR panel under the main screen (omarchyvr-voice listen + `omarchyvr-voice captions`; `--voice-hud` adds the head-locked HUD); the WayVR keyboard is always hidden | yes |
| `desk stop` | `[--grace SECS]` (default 30) | SIGINT voice HUD, listener, WayVR, passthrough (10 s each), then Monado | yes |
| `desk status` | none | pids and `vr status` | read |
| `mount-shared` | none | prints the mount command for the NTFS drive (UUID from `shared_drive_uuid` in `~/.config/omarchyvr/paths.toml`) and says if mounted | no (the mount itself is root) |
| env (vr/desk) | `PIMAX_MONADO_BUILD` (default `~/src/monado/build`), `PIMAX_VR_SESSION_DIR`, `PIMAX_NO_NOTIFY=1`, `XRT_COMPOSITOR_LOG`, `PIMAX_LOG`, `SCALE`, `XRT_COMPOSITOR_SCALE_PERCENTAGE` | | |

Environment variables the launcher and Monado read include `PIMAX_FPS`, `PIMAX_AXIS_MODE`, `PIMAX_ROT_LEFT/RIGHT`, `PIMAX_CANT_DEG`, `PIMAX_LENS_CONF`, `PIMAX_EYE`, `PIMAX_EYE_FLIP_X/Y/Z`, `PIMAX_EYE_CALIBRATION`, `PIMAX_EYE_FILTER`, `PIMAX_EYE_POWER_ON=0`, `PIMAX_EYE_USB_RESET=0`, `PIMAX_SLAM`, `PIMAX_CAM_AE` (default off), `PIMAX_CTRL_HANDS`, `PIMAX_CTRL_AIM=head|level`, `PIMAX_CTRL_REST`, `PIMAX_BIAS_TIMEOUT_MS`, `PIMAX_DEBUG_PTRACE=1`, `PIMAX_PT_HEAD_OFFSET`, `PIMAX_VOICE_SOCKET`, `CLIENT_VK_DEFER_SWAPCHAIN_FREE=0`, `XRT_COMPOSITOR_FORCE_WAYLAND_DIRECT=1`, `XRT_COMPOSITOR_DESIRED_MODE=2`.

### 2.2 `omarchyvr-doctor` (read-only)

| invocation | flags | what |
|---|---|---|
| `omarchyvr-doctor` | `--json`, `--fix-plan` (prints fix commands, never runs them), `--no-pager` | health checks: USB/udev, keepalive, NVIDIA patches (viewport, flip-fix, bounded-wait ids), EDID connector, lease protocol, setcap, kernel log |
| `omarchyvr-doctor info` | `--json` (always JSON) | headset, GPU, connector, profile description |

### 2.3 `omarchyvr-app` (GUI)

`omarchyvr-app [--self-test] [--screenshots DIR [--real]]`: the "Pimax" app (desk mode, setup checklist, tools, logs and history, first-run wizard, profiles), see [GUI.md](GUI.md). `--self-test` collects everything once (read-only) and prints a summary plus the items that need attention; `--screenshots` renders every page to PNGs on the CPU (demo data, or `--real` for this machine). Runs without root; root fixes go through a confirmed pkexec.

### 2.4 `pimax-eye` (Tobii EyeChip)

Exit codes: 0 ok, 1 usage, 2 USB or device error, 3 replay refused or failed, 4 no gaze sample, 5 samples but some failed to decode.

| subcommand | flags | what | effect |
|---|---|---|---|
| `pcap` | `<capture.pcap> [--device N] [--csv FILE] [--head N] [--plan]` | decode a USBPcap capture | offline |
| `plan` | `<capture.pcap> [--device N]` | list the transfers of the Windows start and what the capture cuts | offline |
| `probe` | `[--usb-device PATH] [--claim]` | find the chip in sysfs, check the usbfs node opens | sends nothing |
| `replay` | `[--skip-auth] [--skip-cal] [--skip-vendor-write] [--dry-run] [--seconds S] [--force-replay] [--raw-log FILE] [--usb-device PATH]` | send the 26-step cold-start list (captured bytes only), listen for gaze. Calibration ops 0x3f2/0x407/0x42e/0x3fc/0x44c refused in code | chip |
| `live` | `[--with-cal] [--skip-auth] [--seconds S] [--print-hz F] [--usb-device PATH]` | Monado-side session: gaze in the OpenXR head frame, reconnects. `--with-cal` sends blob 0x456 (do not use without approval) | chip |
| `watch` | `[--with-cal] [--skip-auth] [--seconds S] [--hz F] [--log FILE.csv] [--usb-device PATH]` | full-screen live view of pupil and BAD flags. Needs the chip free | chip |
| `ipd` | `[--seconds S] [--apply [--mm X] \| --watch] [--usb-device PATH]` | measure IPD (field 34 and origin distance). `--apply` sends the captured set-IPD command `58 06 5c 05 01 lo hi` ONCE, clamped 58-72 mm, then verifies. `--watch` is the live view with keys a / + / - / q | headset motor (approved) |
| `power-on` | `[--wait S]` | if headset enumerated and chip absent, sends `58 05 3c 05 00 01` once and waits (default 15 s) | headset (approved) |
| `stream` | `[--capture FILE] [--capture-device N] [--allow-incomplete] [--skip-op OPS] [--force-replay] [--dry-run] [--seconds S] [--print \| --print-all] [--stop] [--usb-device PATH]` | replay the start from a capture, then decode gaze for S s | chip |

### 2.5 `pimax-eyecal` (OpenXR app, needs the eye build)

`pimax-eyecal [--points 9\|13] [--pursuit] [--recenter] [--flip-x 0\|1] [--flip-y 0\|1] [--head-limit DEG] [--res-div N] [--calibration PATH] [--no-raw-log]`; `pimax-eyecal --simulate [--seed N] [--points 9\|13] [--pursuit] [--calibration PATH]`. Keys: space/a next, r redo, y save, p pursuit, c cursor, s runtime check, q quit. Writes `~/.config/omarchyvr/eye-calibration.toml` (old kept as .bak-<utc>) and `~/.local/share/omarchyvr/eye-calibration/eyecal-<utc>.json`. Opens the chip directly: stop WayVR or desk mode first.

### 2.6 `pimax-cam`

Run as `pimax-cam <cmd>`; live sources need the headset lock.

| subcommand | args and flags | what |
|---|---|---|
| `info` | `[--source S] [--frames N]` | device, formats, first frame metadata |
| `stats` | `[--source S] [--seconds N] [--fps F]` | capture rate, lost frames |
| `record` | `OUT.pxcam [--seconds N] [--fps F] [--zlib L] [--frames N] [--source S]` | raw recording |
| `tiles` | `SRC --out DIR [--index N] [--kind tracking\|led\|any] [--count K]` | split tiles to PNG |
| `rectify` | `SRC --out DIR [--pair A,B] [--index N] [--conv 1\|2] [--kb6] [--focal F] [--calib XML] [--overlay F]` | stereo rectification check |
| `hypotheses` | `SRC [--frames N] [--calib XML] [--overlay F]` | rank calibration readings by epipolar error |
| `calib` | `[XML] [--conv 1\|2] [--overlay F]` | parsed calibration |
| `cpu` | `PID [--seconds N]` | per-thread CPU of a process |
| `import` | `IN.raw OUT.pxcam [--zlib L]` | v4l2 YUYV dump to recording |
| `broker` | `[--source S] [--fps F] [--socket P] [--slots N] [--loop]` | one camera stream shared by many readers (`--source broker[:SOCKET]` to read it) |

SRC: empty or `auto` (live), `/dev/videoN`, `broker[:SOCKET]`, or a `.pxcam` file. `--overlay F` applies the selfcal overlay (`default` = the cam-calib-overlay.json).

### 2.7 `omarchyvr-passthrough`

| subcommand | flags | what |
|---|---|---|
| `render` | `--out DIR [--tiles PREFIX \| --source SRC [--index N]] [--depth M\|inf] [--size WxH] [--fov L,R,U,D] [--ipd MM] [--head-offset X,Y,Z] [--gamma G] [--calib XML] [--overlay FILE\|default] [--no-sweep]` | offline per-eye images and coverage maps |
| `run` | `[--source SRC] [--depth M\|inf] [--res-div N] [--cam-delay-ms D] [--gamma G] [--gain K] [--seconds S] [--off] [--head-offset X,Y,Z] [--calib XML] [--stats S] [--overlay FILE\|default]` | live OpenXR client. Needs monado-service and the camera free (no PIMAX_CAM, no PIMAX_HANDS=mercury). Keys: p on/off, +/- depth, [ ] camera delay, g gamma, f freeze, q quit |
| `rig` | `[--calib XML] [--overlay FILE\|default] [--head-offset X,Y,Z]` | camera geometry in the head frame |

### 2.8 `omarchyvr-slam` (offline 6DoF tools)

`export [--out DIR] [--calib XML] [--cams 0,1] [--imu F] [--base CONFIG.json] [--gui] [--cam-offset-ns N]` (writes Basalt calib, VIO config, SLAM_CONFIG toml into `~/.config/omarchyvr/slam`); `check DATASET [--no-files]`; `imu-noise DATASET [--window S] [--inflate K] [--write F]`; `timing REC.pxcam [--frames N]`; `ate EST.csv REF.csv [--est-format euroc\|pimax-windows] [--ref-format ..] [--max-gap-ms M] [--rpe-s D]`; `loop EST.csv`; `stats TRAJ.csv [--format F] [--vs TRAJ2 [--vs-format F]]`; `simulate TRUTH.csv [--format pimax-windows] [--noise-mm 2] [--latency-ms 25] [--tau-ms 150] [--dropout A:B]`; `ae REC.pxcam [--frames N]` (dry run, sends nothing); `shift DATASET OUT --cam-offset-ns N`; `smoke-euroc REC.pxcam OUT [--cams 0,1] [--seconds S]`.

### 2.9 `omarchyvr-room` (offline room memory)

`build DATASET TRACKING.csv OUT_DIR [--calib XML] [--every N]`; `reloc MAP_DIR DATASET TRACKING.csv [--calib XML] [--every N] [--save]`; `info MAP_DIR`; `anchor MAP_DIR list \| set NAME X Y Z [YAW_DEG] \| rm NAME`; `demo`.

### 2.10 `omarchyvr-voice` and `omarchyvr-voice-hud`

`omarchyvr-voice listen [--device NAME\|default] [--input FILE.wav\|-] [--fast] [--socket PATH\|none] [--json] [--levels] [--threads N (2)] [--model DIR] [--lib PATH] [--vad silero\|silero-gate\|energy] [--vad-model PATH] [--vad-threshold 0.5] [--endpoint-ms 480] [--partial-ms 640] [--max-s 20] [--intents FILE\|none] [--engine parakeet\|whisper] [--whisper-model FILE] [--gpu] [--lang en]`; `transcribe FILE.wav... [--json]`; `devices`; `tail [--json]`; `pause \| resume \| ping`; `bench [--clips DIR] [--threads 1,2,4] [--partial-ms ..] [--out FILE.md] [--gap 1.5] [--noise 0.002]`; `vad FILE.wav [--vad silero\|energy]`; `info`. Socket: `PIMAX_VOICE_SOCKET` or `$XDG_RUNTIME_DIR/omarchyvr-voice.sock`.

`omarchyvr-voice-hud [run] [--socket PATH] [--distance M] [--width M] [--down-deg DEG] [--lines N] [--keep N] [--timeout SECS] [--font PATH] [--font-size PX] [--placement N] [--idle-dot] [--format srgb\|unorm] [--demo] [--once]`; `preview OUT.png [--text T] [--partial T] [--status S --detail T] [--speech] [--level DB] [--backdrop RRGGBB]`; `dump [--seconds S] [--raw]`.

### 2.11 `pimax-lens` and `pimax-lenstest`

| tool | subcommands and flags | what |
|---|---|---|
| `pimax-lens` | `show [--conf PATH]`; `grid [--conf PATH] [--out DIR] [--div N]`; `fit-pvr <dump.json> [--out FILE] [--conf FILE] [--dry-run] [--inlier-deg D] [--per-channel]`; `make-lut <dump.json> --out FILE`; `fit [--terms N] [--rmax R] [--window N] [--table FILE] [--cont-weight W] [--v-down V]` | inspect and fit lens models, no headset |
| `pimax-lenstest` (run) | `[--pattern NAME] [--patterns A,B] [--eye both\|left\|right] [--auto SECS] [--seconds S] [--format srgb\|unorm] [--eye-axis] [--no-hud] [--no-hud-quad] [--square-size DEG] [--square-dir right\|left\|up\|down] [--lens-conf F] [--timing] [--swim-cpu] [--gaze-points 13\|17] [--gaze-out PATH] [--res-div N] [--capture DIR]` | OpenXR test patterns in the headset |
| `pimax-lenstest` (no headset) | `list`; `config [--lens-conf F]`; `preview DIR [--width W] [--height H] [--fov L,R,U,D]`; `bench-swim [--width W] [--height H] [--frames N]`; `simulate DIR [--lens-conf F] [--div N]`; `compare CAPTURE_DIR SIM_DIR`; `dump2png RAW OUT.png [--down N] [--rot-left D] [--rot-right D]`; `dumpdiff A.raw B.raw [--png OUT] [--down N]` | |

Patterns: grid, rings, channels, fov-edge, square100, square25, corners, align, swim, flat, gaze.

### 2.12 `pimax-selfcal`

`estimate SCENES... [--pairs 0,1;0,2;1,3;2,3\|all] [--reference 0] [--free 1,2,3] [--rounds 2] [--calib XML] [--from OVERLAY\|default] [--out DIR] [--holdout] [--no-eye] [--all-pairs] [--accept-sigma PX] [--prior-deg D] [--write [FILE\|default]] [--force]`; `check SCENES... [--overlay [FILE\|default]]`; `pairs SCENES... [--out DIR]`; `lens SCENES...`; `selftest [--out DIR] [--angle DEG]`; `show [FILE\|default]`; `rollback [FILE\|default]`; `clear [FILE\|default]`. Scenes: directory of tile PNGs, a tile prefix, or `FILE.pxcam[@FIRST[:COUNT[:STEP]]]`. Offline only; `--write` installs the overlay (old kept as .prev) only if the sanity gate passes.

### 2.13 OpenXR test clients

| tool | flags | what |
|---|---|---|
| `xr-handtest` | `--source any\|both\|controller\|left\|right`, `--seconds`, `--hz`, `--min-valid`, `--no-grip-check`, `--require`, `--require-grip`, `--demo`, `--print-hz`, `--quiet`, `--wait-runtime` | headless hand-tracking check (exit 0 pass, 1 check failed, 2 cannot run) |
| `xr-gaze` | none needed | prints the eye gaze pose at 10 Hz |
| `xr-pinchtest` | `--demo` | headless check that controllers work as XR_EXT_hand_interaction pinch |

### 2.14 `omarchyvr-desktest`, `omarchyvr-voicebench`

`omarchyvr-desktest metrics FILE [--trim S] [--window T0,T1]`; `info RAW`; `png RAW OUT.png [--crop x,y,w,h] [--eye left\|right] [--down N] [--up N]`; `black RAW --crop ..`; `edge RAW --crop .. [--chan g]`; `ladder RAW --crop .. --period P [--kmin 0.8] [--kmax 2.0]`. `omarchyvr-voicebench bench [--lib PATH] [--models DIR] [--threads 4] [--reps 5] [--warmup 1] [--pad 0.3] [--out DIR] [--no-tts-es] [--skip-stt]`; `idle`; `resample IN.wav OUT.wav RATE`; `info`.

### 2.15 `omarchyvr-roommap` (persistent room map)

Maps the room from a 6DoF session, recognises it in a later session and writes the Monado world offset, so the world origin and the WayVR windows come back to the same real place without a printed tag. Basalt (the VIO in Monado) starts a new world at every start and cannot save a map, so this tool keeps its own: stereo ORB keyframes in room coordinates. Design, numbers and failure modes: `analysis/room-map.md`. Nothing is sent to the headset.

| subcommand | flags | what |
|---|---|---|
| `build` | `(--rec DIR [--tracking CSV] \| --source broker [--seconds 60]) [--name NAME \| --map DIR] [--every N] [--kf-dist M] [--kf-angle DEG] [--no-loops] [--refine [--keep-unrefined]]` | map the room: stereo ORB keyframes (spacing `--kf-dist` M and `--kf-angle` DEG, defaults 0.20 and 15), loop closure inside the session (4-DoF pose graph; `--no-loops` skips it), optional multi-view point refinement (`--refine`). Live: walk slowly, look at every wall, the desk and the floor near you, and come back past the start so the loop closes |
| `relocalize` (alias `reloc`) | `(--rec DIR [--tracking CSV] \| --source broker [--seconds 15]) [--name NAME \| --map DIR] [--every N] [--window S] [--threads N] [--result FILE] [--apply]` | recognise the room: `room_from_slam` (yaw + translation) pooled over the frames of a window (`--window`, default 4 s; a fix needs 12 agreeing frames and 300 inliers). Offline it also prints the whole-session fit and the per-window consistency. `--apply` also writes the Monado offset |
| `apply` | `[--name NAME \| --map DIR] [--result FILE] [--pid N] [--dry-run]` | write the Monado world offset from `last-reloc.toml` or `--result`. Refuses a live result that belongs to another Monado pid, and an offset over 20 m. `--dry-run` writes nothing |
| `info` | `[--name NAME \| --map DIR]` | map summary (sessions, keyframes, points, extent), `roommap.toml`, last relocalisation |
| `cycle` | `RESULT.toml RESULT.toml [RESULT.toml ...]` | compose relocalisation results around a cycle of sessions (A->B, B->A, ...); identity = consistent |
| `eval` | `--map-a NAME --rec-a DIR --map-b NAME --rec-b DIR [--every N] [--window S] [--threads N]` | same-place cycle test of two recorded sessions, each with its own map (offline validation; not in the usage text) |

Common: `--calib XML` (default: the headset calibration) for `build`, `relocalize` and `eval`; `--name` defaults to `home`; `--threads N` defaults to 8; `--every N` uses every Nth frame set.

Input. Offline: a recording of `analysis/tools/run_slam.sh live\|record` and its Basalt trajectory (`run_slam.sh replay DIR` writes `DIR/replay-*/tracking.csv`; the newest is used). Live: `pimax-cam broker` frames and Monado's head ring. The ring needs 6DoF and the anchor hook (2.16), for example `omarchyvr desk start --6dof --anchor`.

Output. The map directory `~/.local/share/omarchyvr/roommap/<name>/` (`PIMAX_ROOMMAP_DIR` changes the base): `map.bin`, `roommap.toml`, `correction.csv`, `reloc-<source>.toml`, `last-reloc.toml`. `apply` and `relocalize --apply` write `$XDG_RUNTIME_DIR/pimax-anchor.offset`, the file Monado reads.

Use. Map once (live, about 60-90 s). After each 6DoF start run `relocalize --source broker --name home --apply` and look around for a few seconds (the first fix came after 3-7 s in the offline tests). A map built offline has the Basalt replay world of its recording as its room frame, so after the first `apply` place the WayVR windows once by hand. From then on they come back. Pimax app: Room page, developer mode (Map my room runs `build --source broker --seconds N --name NAME`, default 90 s and `home`; Recognise room runs `relocalize --source broker --name NAME --apply`). Auto mode (`omarchyvr-autod`) runs the same relocalize after a 6DoF start when the map `home` exists.

### 2.16 `omarchyvr-anchor` (AprilTag room anchor)

A printed AprilTag (tag36h11) on the desk is a fixed reference that brings the VR windows back to the same real spot every session. The tool finds the tag in the fisheye tiles of `pimax-cam broker`, refines its pose on the KB4 reprojection error over all cameras that see it, and combines it with the head pose Monado reports at the same instant. Print `analysis/anchor/tag36h11-id0-150mm.pdf` (A4, 100 %, the black square must measure 150 mm), glue it flat on a board, put it on the desk 40-70 cm in front of you in good light and do not move it. Details: `analysis/anchor/README.md`.

| subcommand | flags | what |
|---|---|---|
| `tag` | `[--id 0] [--size-mm 150] [--out-dir DIR]` | write the printable SVG + PDF (`tag36h11-id<N>-<size>mm.svg` and `.pdf`) |
| `detect` | `[opts]` | tag pose in the head frame (and in the Monado world when Monado runs with `PIMAX_ANCHOR=1`) |
| `calibrate` | `[opts] [--window NAME=px,py,pz,qx,qy,qz,qw ...]` | save the reference (tag pose in the app world) to `~/.config/omarchyvr/anchor.toml`. Run it once with the windows where you want them, headset held still, tag in view for 4 s. `--window` also records a window pose relative to the tag |
| `apply` | `[opts] [--full] [--dry-run]` | measure the tag now and write the Monado world offset. Yaw + translation only by default (the SLAM world is gravity aligned); `--full` also corrects roll/pitch; `--dry-run` writes nothing |
| `status` | none | config, head ring, offset file |
| `clear` | none | remove the offset (identity) |

Common options: `--source broker\|FILE` (default broker), `--seconds N` (default 4), `--cams 0,1,2,3`, `--size-mm N` (default: config or 150), `--id N`, `--calib XML`, `--extrinsics JSON` or `--factory`, `--png OUT` (debug mosaic of the last frame). `calibrate` and `apply` refuse (nothing saved or written) when the tag is not seen with a head pose from Monado, or when the samples are too few or too scattered. `apply` also refuses an offset over 6 m. Use: `calibrate` once; in every later session, after the headset tracks (6DoF), look at the tag and run `apply`.

Files. Reads: `pimax-cam broker` frames (or a recording with `--source FILE`), the camera calibration, `~/.config/omarchyvr/ctrl-cam-extrinsics.json` (unless `--factory`), `~/.config/omarchyvr/anchor.toml` (`PIMAX_ANCHOR_CONFIG` changes the path) and Monado's head ring. Writes: `anchor.toml` (`calibrate`), `$XDG_RUNTIME_DIR/pimax-anchor.offset` (`apply`; `clear` removes it), the SVG and PDF (`tag`), the debug PNG (`--png`).

The anchor hook (shared with `omarchyvr-roommap`, 2.15). Start Monado with `omarchyvr desk start --anchor` or `omarchyvr vr start --anchor`: the launcher then sets `PIMAX_ANCHOR=1`. This needs the `desk` Monado build with the hook (branch `desk` in `~/src/monado-eye`; `setcap` again after the relink). Monado then writes its raw head poses (before the offset) to the ring `$XDG_RUNTIME_DIR/pimax-anchor.head`. It applies the offset file `$XDG_RUNTIME_DIR/pimax-anchor.offset` to the head pose (`app_from_slam = offset * slam`), so controllers, gaze and the WayVR stage with its windows follow. The offset file is text: `pimax-anchor-offset 1`, `pid N`, `pose px py pz qx qy qz qw`. It is per Monado run: Monado deletes it at start and ignores one with another pid. `PIMAX_ANCHOR_HEAD_FILE` and `PIMAX_ANCHOR_FILE` override the two paths.

One writer per session. Only one of `omarchyvr-roommap` and `omarchyvr-anchor` should write the offset in a Monado session; the two CLIs do not enforce it. The Pimax app records the writer in `$XDG_RUNTIME_DIR/omarchyvr-anchor.offset.owner` (`roommap <pid>` or `anchor <pid>`) and refuses the other one until the offset is cleared. Auto mode only uses `omarchyvr-roommap`. Pimax app: the Room page (developer mode) has the hook switch (`--anchor` at the next start), Room map (Map my room, Recognise room) and AprilTag (Calibrate, Apply, Clear offset, Printable tag).

### 2.17 `omarchyvr-perf` (wrist performance readout)

`omarchyvr-perf run [--file F] [--wayvrctl P] [--overlay NAME] [--interval-ms N]`. F defaults to `$PIMAX_PERF_FILE`, else `$XDG_RUNTIME_DIR/omarchyvr-perf.stats`; Monado must run with `PIMAX_PERF_FILE=F`. Other defaults: `--wayvrctl ~/src/target-desk/release/wayvrctl`, `--overlay perf`, `--interval-ms 500` (100-5000). It runs until stopped.

It reads the key=value stats file that Monado (`~/src/monado-eye`, branch `desk`, `comp_multi_system.c`) writes every 0.5 s. It sets text and colour of the four labels of the WayVR panel `perf` (defined by the launcher's `gui/perf.xml`; on the back of the right wrist, not grabbable) through a long-running `wayvrctl batch`. Lines: `HMD 72/72` (compositor fps against the 72/90 target), `APP 71/72` (main app fps), `miss N /5s` (late or skipped compositor frames in the last 5 s), `cpu x gpu y` (app CPU and GPU ms). Rates are green from 95 % of the target, yellow under 95 %, red under 85 %. Times are green under 80 % of the frame period, yellow under 100 %, red beyond. The panel is grey (`perf: no data`) when the file is missing or older than 2.5 s. Lines are re-sent on change and at least every 2 s. The tool reads the stats file and writes no files. Every 5 s it also writes a frame-health line to stderr (the session's `perf.log`): `perf: unix=.. mono_ms=.. fps=.. frames+N dropped+N late+N app_fps=.. app_gpu_ms=..` (docs/RENDER-QUALITY.md, "Shaky desk screens").

Launcher. `omarchyvr desk start` runs it by default and sets `PIMAX_PERF_FILE` for Monado (a value you set wins); `--no-perf` turns it off and `--perf-bin P` picks the binary. `omarchyvr vr start --perf` does the same for games, with a second minimal WayVR (`--openxr --headless`: an `XR_EXTX_overlay` session without screens, watch or keyboard; config home `~/src/desk-out/wayvr-home-perf`); `omarchyvr vr stop` stops them first. Needs the `desk` Monado build with the stats patch (`setcap` again after the relink) and WayVR `b26591a` or later. Game mode is not yet tried with a real game (docs/WAYVR-CONTROLS.md). Pimax app: Display > Performance, switch "Wrist readout" (developer mode; applies at the next start).

### 2.18 `pimax-ipd` (guided IPD test in the headset)

Guided IPD test with a screen in the headset (`omarchyvr-hud`, 2.19). The default mode opens the eye chip itself; both modes read the live lens IPD from the headset info reports (HID interface 2). The only HID write is the approved IPD motor command (`58 06 5c 05 01 <mm*100 LE>`, 58-72 mm): once with `--apply`, paced by the key presses in `manual`.

| invocation | flags | what |
|---|---|---|
| `pimax-ipd` | `[--seconds S] [--apply [--mm X]] [--overlay] [--placed] [--distance M] [--usb-device PATH] [--no-hud]` | shows 'Look straight ahead', a 3-2-1 countdown, the measurement (default 5 s) with a progress bar, then measured IPD, lens IPD and the difference. With `--apply` the lenses move and the result is shown; without it nothing is sent. The result screen stays 5 s (Enter ends it) |
| `pimax-ipd manual` | `[--overlay] [--placed] [--distance M] [--no-hud] [--simulate [--png-dir DIR] [--sim-lens MM]]` | pick the sharpest IPD by hand. The panel shows a sharpness test pattern, the target IPD, the live lens IPD and a status. A/D -/+0.5 mm, W/S +/-1 mm (58-72; arrows work too), Enter saves the lens IPD to `~/.config/omarchyvr/ipd.env` and exits, q exits (Esc does nothing). Keys only change the target; the motor command goes out 700 ms after the last key, never during a move (it waits until the lens is within 0.3 mm or 6 s) and at least 1.5 s after the previous one. No eye tracking: the eye chip is not opened |
| `pimax-ipd --simulate` | `[--png-dir DIR] [--sim-field34 MM] [--sim-origin MM] [--sim-lens MM] [--apply [--mm X]]` | no headset: fake measurement, screens written as PNG files (default DIR `/tmp/pimax-ipd-sim`; `manual --simulate`: `/tmp/pimax-ipd-manual-sim`) |

Options: `--overlay` = `XR_EXTX_overlay` session on top of a running main app (WayVR, desk mode), default is a main app; `--placed` = fix the panel in the room where you looked at start (default: head-locked); `--distance M` = panel distance in metres (default 1.5); `--no-hud` = terminal only; `--usb-device PATH` = usbfs node of the eye chip.

`--apply` REFUSES (nothing sent, 'Measurement looks wrong') when the measurement is outside 58-72 mm or more than 4 mm from the current lens IPD, unless `--mm X` gives the value. The shared rules in `pimax-eye` also refuse it with fewer than 100 good samples, or when IPD field 34 and the origin distance differ by more than 3 mm. Exit codes: 0 ok, 2 refused, failed or usage error, 3 the lenses did not arrive (not within 0.5 mm of the target after 5 s).

Needs. The eye chip can be opened by one process only, so start Monado without eye tracking (`omarchyvr vr start --force` with `PIMAX_MONADO_BUILD=~/src/monado-eye/build`, or `omarchyvr desk start --force` without `--eye`) and run with `XR_RUNTIME_JSON=~/src/monado-eye/build/openxr_monado-dev.json` (for the panel; `--no-hud` is terminal only). Device tool: see the lock rule at the top of this section. Writes `~/.config/omarchyvr/ipd.env` (`OXR_DEBUG_IPD_MM=<mm>`; Monado reads the value once at session start) after a verified `--apply` and on Enter in `manual`. The Pimax app does not call it; its IPD tool runs `pimax-eye ipd --watch`.

### 2.19 `omarchyvr-hud` (in-headset panel library)

Library only, no command. It shows large text lines and a progress bar on a panel a short distance in front of the wearer: an OpenXR (Vulkan) quad layer, CPU-rendered with the built-in 5x7 font on a 1536x768 texture (1 mm per pixel). In main-app mode the panel is the only layer (the rest is black). With `Opts::overlay` it is an `XR_EXTX_overlay` session on top of a running main app (WayVR, desk mode); only one main OpenXR app can own the display.

API: `Hud::new(Opts)` (`XR_RUNTIME_JSON` selects the runtime), `Hud::handle()` (a cloneable, thread-safe handle with `set_lines`, `set_styled`, `set_progress`, `request_exit`) and `Hud::frame()`. `Opts`: `placement` (`HeadLocked`, or `PlacedAtStart` = fixed in the room where the head pointed at start), `distance` in metres (default 1.5), `overlay`, `app_name`. `Sink` is the trait that the handle and the PNG writer of `pimax-ipd` implement; `screen::write_png` renders a `Screen` to a file without a headset. `Screen.pattern` draws a sharpness test pattern (fine text, resolution lines, grid, centred cross). Used by `pimax-ipd`.

### 2.20 Shell launchers (`analysis/tools/`)

Each needs the repo tools built. "Headset" = touches the device or display (needs the lock and a free headset).

| script | headset | purpose (from its header) |
|---|---|---|
| `run_vr_session.sh [logdir]` | yes | long-running Monado for xrizer games, Ctrl-C stops (env SCALE, XRT_COMPOSITOR_DESIRED_MODE) |
| `run_direct_test.sh [outdir]` | yes | direct-mode smoke test, max ~40 s |
| `run_xr_room.sh [outdir]` | yes | world-locked sky room in direct mode (`SECS=60`) |
| `run_vr_scene.sh [outdir]` | yes | head-tracked scene (`SECS=60`) |
| `run_lens_test.sh` | yes | lens test session with pimax-lenstest; SIGINT, 15 s grace before SIGKILL |
| `run_lens_tune.sh [outdir]` | yes | edit lens.conf and look (`SECS`, `FPS`, `PIMAX_LENS_CONF`) |
| `lens_align.sh` | yes | live stereo alignment (align pattern) |
| `lens_swim.sh` | yes | pupil-swim and residual check (swim, grid, flat) |
| `lens_center_sweep.sh [values]` | yes | grid per horizontal lens centre (variants in lens-ipd/) |
| `run_lens_ab.sh [OUTDIR]` | no (capture) | A/B of the Rust vs C lens model, pixel diff |
| `run_ctrl_test.sh [seconds]` | no (windowed) | controller test through OpenXR |
| `run_hand_test.sh [phase...]` | lock, null compositor | controller hand tracking: nodevice, demo, real |
| `run_hands_composite.sh` | no | offline desk-mode composite with demo hands |
| `run_vr_fps_test.sh FPS OUTDIR` | yes (display) | refresh-rate test with dumps |
| `run_vr_color_test.sh` | yes (display) | colour pipeline check, sRGB vs UNORM |
| `run_slam.sh record\|live\|replay [SECS]` | yes | 6DoF record, live pose printer, replay (offline replay has no device) |
| `run_slam_view.sh [SECS]` | yes | live 6DoF walk with camera view (broker + SLAM + passthrough) |
| `eye_accuracy.sh` | yes (eye build) | gaze accuracy test (lenstest gaze pattern) |
| `eye_calibrate.sh` | yes (eye build) | pimax-eyecal against the eye build |
| `vr_runtime_switch.sh xrizer\|steamvr\|status` | no | switches OpenVR and OpenXR runtime config (xrizer / SteamVR restore) |

### 2.21 Python and Windows-side (reference only)

`windows/pimax-dump-distortion.ps1` (Windows, read-only collection), `capture*.ps1` (USBPcap captures on Windows), `tools/` transfer helpers.

---

## 3. Install and dependency steps

Tick "root" when a step needs sudo. A GUI must ask for each root step separately, show the command, and never run it silently.

### 3.1 Build

| step | command | root | output |
|---|---|---|---|
| Rust workspace | `cd ~/pimax-re/rust && cargo build --release` | no | `rust/target/release/` (pimax, omarchyvr-doctor, omarchyvr-app, pimax-eye, pimax-lenstest, omarchyvr-passthrough, xr-handtest, xr-gaze, ...) |
| Tests | `cargo test --workspace`, `cargo clippy --workspace --all-targets` | no | 62+ tests, no hardware |
| Install CLI | `install -Dm755 rust/target/release/omarchyvr ~/.local/bin/omarchyvr` | no | `~/.local/bin/omarchyvr` |
| Monado pimax | `cd ~/src/monado && meson setup build && ninja -C build` (or the project's build recipe); `PIMAX_USE_RUST_LIBPIMAX=ON` needs `libpimax_capi` | no | `build/src/xrt/targets/service/monado-service` |
| Monado eye | same in `~/src/monado-eye` (branch eye) | no | `~/src/monado-eye/build/...monado-service` |
| Monado slam | same in `~/src/monado-slam` (branch slam, needs Basalt in `~/.local`) | no | `~/src/monado-slam/build/...monado-service` |
| WayVR | `cargo build --release` in `~/src/wayvr` (eye-gaze branch) with target dir `~/src/target-desk` | no | `~/src/target-desk/release/wayvr`, `wayvrctl` |
| omarchyvr-perf | `cargo build --release -p omarchyvr-perf` in `rust/` | no | `rust/target/release/omarchyvr-perf` (wrist readout, see docs/WAYVR-CONTROLS.md; needs Monado `PIMAX_PERF_FILE`) |
| Passthrough | `cargo build --release -p omarchyvr-passthrough` (target dir `~/src/target-passthrough`) | no | `omarchyvr-passthrough` |
| Voice | `cargo build --release -p omarchyvr-voice -p omarchyvr-voice-hud` (target-voice, target-voicehud) | no | `omarchyvr-voice`, `omarchyvr-voice-hud` |
| Basalt | built in `~/src/basalt`, installed to `~/.local` | no | libraries |
| xrizer | built from `~/src/xrizer` | no | OpenVR-to-OpenXR layer |
| C test clients | built by hand from `analysis/tools/*.c` (see headers) | no | binaries |

### 3.2 Capabilities (after every relink)

| step | command | root |
|---|---|---|
| GPU priority for the service | `sudo setcap cap_sys_nice+ep <monado-service path>` (do it for `~/src/monado` and `~/src/monado-eye`; monado-slam has NO cap today) | **root** |

Every relink drops the capability. `omarchyvr vr start` warns and prints this line when it is missing. Check with `getcap <path>`.

### 3.3 Headset access

| step | how | root |
|---|---|---|
| udev rule | `sudo install -Dm644 linux/70-pimax.rules /etc/udev/rules.d/70-pimax.rules && sudo udevadm control --reload && sudo udevadm trigger` (installed on this machine) | **root** |
| user keepalive | `ln -s ~/pimax-re/linux/omarchyvr-keepalived.service ~/.config/systemd/user/` then `systemctl --user daemon-reload && systemctl --user enable --now omarchyvr-keepalived` | no |
| drop-in (Rust) | `~/.config/systemd/user/omarchyvr-keepalived.service.d/rust.conf` with `ExecStart=` (clear) and `ExecStart=%h/.local/bin/omarchyvr keepalived` | no |
| Rust unit template | `rust/systemd/omarchyvr-keepalived-rust.service` (not installed; superseded) | — |
| eye chip USB reset | done by the tool via usbfs, no sudo (uaccess via udev) | no |

### 3.4 NVIDIA display path

| step | command | root |
|---|---|---|
| Boot EDID | `sudo install -D -m644 linux/pimax-dp-edid-msvsdb.bin /etc/pimax/dp-edid.bin` (installed; byte-identical). The native variant `linux/pimax-dp-edid-native.bin` gives modes but not non-desktop | **root** |
| modeset config | `echo 'override.[0000:01:00.0].DP-0=/etc/pimax/dp-edid.bin' \| sudo tee /etc/pimax/nvidia-modeset.conf` (installed) | **root** |
| modprobe | `echo 'options nvidia-modeset config_file=/etc/pimax/nvidia-modeset.conf' \| sudo tee /etc/modprobe.d/nvidia-pimax.conf` (installed) | **root** |
| initramfs | `echo 'FILES+=(/etc/pimax/nvidia-modeset.conf /etc/pimax/dp-edid.bin)' \| sudo tee /etc/mkinitcpio.conf.d/pimax.conf` and `sudo mkinitcpio -P` (installed; on Omarchy use `limine-mkinitcpio`), then reboot | **root** |
| NVIDIA patches | `linux/nvidia-patch-reapply.sh --check` (no root). Apply and rebuild: `sudo linux/nvidia-patch-reapply.sh --rebuild` (patches `/usr/src/nvidia-<ver>`, dkms build, boot image, verify) | **root** |
| Pacman hooks | `sudo env L=$HOME/pimax-re/linux bash -c 'install -Dm755 $L/nvidia-patch-reapply.sh /usr/local/lib/omarchyvr/nvidia-patch-reapply.sh && for p in nvidia-2h1or-viewport nvidia-2h1or-flip-fix nvidia-evo-makeroom-bounded-wait; do install -Dm644 $L/$p.patch /usr/local/lib/omarchyvr/$p.patch; done && install -Dm644 $L/nvidia-pimax-patch.hook /etc/pacman.d/hooks/65-nvidia-pimax-patch.hook && install -Dm644 $L/nvidia-pimax-verify.hook /etc/pacman.d/hooks/75-nvidia-pimax-verify.hook'` (installed on this machine) | **root** |
| Undo hooks | `sudo rm /etc/pacman.d/hooks/65-nvidia-pimax-patch.hook /etc/pacman.d/hooks/75-nvidia-pimax-verify.hook; sudo rm -r /usr/local/lib/omarchyvr` | **root** |
| Check | `cat /sys/module/nvidia_modeset/parameters/merged_head_sync` must print `1` (it does now) | no |

The patches must be re-applied after every nvidia-open-dkms upgrade (the pacman hook does this when installed).

### 3.5 EDID variants (for reference)

`linux/pimax-dp-edid-native.bin` (8 bpc, 7104x3840 @72/@90, desktop), `pimax-dp-edid-msvsdb.bin` (native plus Microsoft HMD VSDB: non-desktop, installed), `pimax-dp-edid-hmd.bin`, `pimax-dp-edid-hmd-dual.bin` (DisplayID 2.0 HMD, dead end), `pimax-dp-edid-linux.bin`, `analysis/dp-edid-*.bin` (1080p, 4k, tall, wide, norange tests). Never change a live EDID with `echo off/detect` (see Safety).

### 3.6 Lens, eye, camera and voice config files

Created by the tools on demand; see 1.5 for paths. No root needed for any of them. `~/.config/omarchyvr/` is per user.

### 3.7 Desktop entry

`linux/omarchyvr-vr.desktop` (template with `@OMARCHYVR@` placeholder): entries "Pimax VR" (`omarchyvr vr start --wait`), "Start Pimax VR at 90 Hz", "Stop Pimax VR", "Pimax VR status". Replace `@OMARCHYVR@` with the path to `omarchyvr` (e.g. `~/.local/bin/omarchyvr`) when installing into `~/.local/share/applications/`. Root: no.

### 3.8 Voice models

Download the Parakeet, VAD and optional Whisper models into `~/models/` (no root). The source location of these is not in the repo; record the URL or checksum when the GUI downloads them.

### 3.9 Shared drive

`omarchyvr mount-shared` prints the mount command for the NTFS drive by UUID `<shared_drive_uuid>` (`shared_drive_uuid` in `~/.config/omarchyvr/paths.toml`). Mounting is root. Never mount by device name (NVMe names swap between boots).

### 3.10 Summary: root needed?

| step | root |
|---|---|
| Rust build, test, `~/.local/bin/omarchyvr` install | no |
| Monado, WayVR, Basalt, xrizer, voice builds | no |
| setcap on monado-service | **yes** |
| udev rule | **yes** |
| user keepalive service, drop-in | no |
| EDID files, modprobe, mkinitcpio | **yes** |
| NVIDIA patch apply, dkms, boot image | **yes** |
| pacman hooks | **yes** |
| mount NTFS drive | **yes** |
| `omarchyvr vr start/stop/status/desk`, `omarchyvr-doctor`, `pimax-eye` tools | no (device access by uaccess) |
| `omarchyvr vr reset-headset` | no (does not clear NVIDIA) |

---

## 4. Problems and fixes history

Dates are local; "n/a" = no separate commit in this repo (the fix lived in the Monado fork or the root-installed files, or is recorded only in HISTORY.md). Commit ids are from `git log` of this repo unless prefixed with a fork name.

| date | problem | root cause | fix | commit |
|---|---|---|---|---|
| 2026-10-13 am | Windows captures needed to learn the protocol | no Linux info yet | USBPcap captures (power-on, cold start, eye, controllers) | a17e616, ba86a63 |
| 2026-10-13 pm | Headset drops off USB every ~5.4 s (VM test) | first thought VMware USB polling | not a VMware issue | n/a |
| 2026-10-13 18:20 | same detach loop on bare metal | firmware detaches without keepalive | wake + 0x39 keepalive; IF1-IF3 hidraw must be open and drained | 3a56e4f (Rust); user service 18:24 |
| 2026-10-13 18:20 | hidraw permission | no access without root | udev rule with uaccess | 70-pimax.rules |
| 2026-10-13 18:40 | nvidia-drm shows 0 modes | stock EDID; DisplayID colour depth undefined, NVIDIA sends 18 bpp | boot EDID override (8 bpc, byte 20 = 0xA5), via nvidia-modeset config_file | n/a (root files, `linux/pimax-dp-edid-*.bin`) |
| 2026-10-13 19:00 | black panel with a 24 bpp mode | 7104x1840 at 24 bpp enables amps but no picture | 8 bpc EDID | n/a |
| 2026-10-13 19:00 | forced `status off` desyncs Hyprland | EINVAL on DP-1 commits | forbidden; reboot after | n/a (rule) |
| 2026-10-13 20:00 | 7104x3840 rejected | "DP Bandwidth check failed": DSC needed, sink advertises DSC 1.2 | native EDID with 7104x3840 modes validated by nvkms_why | cd0c4a3 (analysis) |
| 2026-10-13 21:00 | Desktop non-desktop not set; Hyprland puts windows in the headset | EDID lacks HMD signal | Microsoft HMD VSDB in CTA block | 4505486, b5fff80 |
| 2026-10-13 21:03 | left half of the frame in both eyes | NVIDIA nvSetViewPortsEvo sets viewport x = 0 on every head of a 2Heads1OR merge (PR #1262) | `linux/nvidia-2h1or-viewport.patch` | 5b778e9, 0f3d233 |
| 2026-10-13 21:50 | controllers in OpenXR | not decoded in Monado | Oculus Touch profile in the driver | 7330c8f, 895c792, b5cdc7e (decoder) |
| 2026-10-13 21:50 | SteamVR rewrites runtime config | SteamVR install | `vr_runtime_switch.sh xrizer` + chmod 444 | e39feda |
| 2026-10-13 22:30 | Dungeons of Eternity: dark, frozen | libcjson missing in container | bundled cJSON in Monado build | 414852f (Monado) |
| 2026-10-13 23:00 | Proton game causes NVIDIA Xid 31 and wedged display | wineopenxr destroys a swapchain while the GPU draws; Monado freed it at once | defer freeing destroyed swapchain images until a fence signals (patch 0010) | fcb6c6a, Monado 1da3626 |
| 2026-10-13 23:xx | headset frozen with sRGB swapchains | NVIDIA "Idling display engine timed out" after unclean restart, not sRGB | none needed; reboot rule | 4c5d31e |
| 2026-10-13 22:48 | Rust port needed for the keepalive and libpimax | C library in Python-era code | rust workspace, same C ABI, 0 diffs | a675a8c, 3a56e4f |
| 2026-10-14 00:41 | WayVR at exit: Xid 31 | dmabuf capture at exit | `capture_method: screencopy` in WayVR config; do not SIGINT WayVR while others run | 34d6ef0; desk.rs 309 |
| 2026-10-14 00:29-00:40 | camera session lock leak | flock/sleep lock leak | fixed by overnight cam agent | cb32307, 6d20ee0 |
| 2026-10-14 00:57 | a background agent ran `limine-mkinitcpio` as root | passwordless sudo exists | rule: no auto-elevating commands | n/a (rule) |
| 2026-10-14 02:00 | overnight lens, hands, cam in one build | merge | Monado patches 0011-0017 | f9d8ef6, f1fbf0a |
| 2026-10-14 06:25 | worn session froze after ~20 s | gyro calibration never finished when worn, so the head pose stayed frozen | 3 s timeout `PIMAX_BIAS_TIMEOUT_MS`, cached bias | d9bab46, Monado d544d3a (patch 0018) |
| 2026-10-14 07:00 | freeze with hands on, then off | lazy hands feed and unbounded GPU waits | lazy controller-hands feed and bounded compositor GPU waits (diagnostic, not the root cause) | be4d119, e64d487 (Monado) |
| 2026-10-14 07:00-09:00 | display freeze: flip never completes (render stuck in vkQueueWaitIdle) | NVKMS applies one flip's acquire/release semaphores to both merged heads (2H1OR); latch splits (most likely) | `linux/nvidia-2h1or-flip-fix.patch` (`merged_head_sync=1`) plus Monado flipsafe (self-revoke and `_exit` on a stuck flip) | ea21a4a, Monado 02889c9; analysis 9825511 |
| 2026-10-14 06:25-09:00 | freeze again after SIGKILL | SIGKILL during stuck flip skips postclose revoke (interruptible) | SIGINT and wait; run_lens_test.sh 15 s grace; `omarchyvr vr stop` saves stacks before SIGKILL | 4641b89, e611580 |
| 2026-10-14 ~11:00 | display freeze fixed | (verified) | 60 s and 300 s worn runs, no freeze | flip-fix + flipsafe (see above) |
| 2026-10-14 ~11:15 | double far lines, single near lines | display canting 2 deg per eye | `cant_deg` in lens.conf (Monado `cant`), default 2.0 | 7c3831a, e09d81d, 4ec0348 |
| 2026-10-14 08:00-09:00 | centre mismatch: double red cross | lens centres not matched | `lens_align.sh`, align pattern; centre 0.1060 was wrong (cut FOV) and was dropped | 9b38563, 33efd16 (obsolete) |
| 2026-10-14 11:30 | fish-eye swim when moving the head | lens curve | swim patterns (GPU), dk2 mid-zone term | c6389ef, a9270ab |
| 2026-10-14 ~13:30 | lens curve from our own model swam or fished when the head moved | own model not matched to Pimax's data | fit to Pimax's real dump (`fit-pvr`), default lens.conf | 54e84bc |
| 2026-10-14 13:30 | exact LUT looked worse than the fit | unexplained (LUT orientation/centre/scale or smoothing) | LUT mode kept as option; decision: fit stays, re-judge in games | ba0c0f0 |
| 2026-10-14 ~15:00 | right eye red tint | colour pipeline | gone after the lens and colour work (no single fix commit) | n/a |
| 2026-10-14 ~14:00 | eye tracking: no gaze | cold-start auth (0x76c challenge, 0x777 answer, not derivable) | replay of the Windows start without auth and calibration: 119 Hz gaze | 1e23144, 9b1854f |
| 2026-10-14 afternoon | eye gaze not reaching OpenXR | no Monado eye path | eye branch of Monado, FLIP_X needed | 1025754 (Monado), ac00a7c |
| 2026-10-14 14:01 | `pimax-eye live` deadlock | tracker thread eprintln! held the stderr lock | removed the stderr lock in the CLI | 63d24c5 |
| 2026-10-14 14:13 | eye module stays off after headset power cycle | eye module needs enable `58 05 3c 05 00 01` | `pimax-eye power-on`, then automatic enable (once per enumeration, chip absent > 5 s) | 540130f, 1897adc |
| 2026-10-14 ~15:45 | eye accuracy 8.8 deg mean error, about 10 deg offset, gain 0.6 | no calibration; sensor frame and offset | host-side correction file and accuracy pattern (raw test only); diagnosis in analysis | 0be3ec8, c1eee9a |
| 2026-10-14 ~15:45 | sending calibration blob 0x456 stopped the chip (0x547 "wedge") | device-side calibration op | calibration ops refused in code; USBDEVFS_RESET recovery after 2 failures (at most once per 60 s) | 1897adc |
| 2026-10-14 afternoon | eye error per eye and drift | one global offset is not enough | per-eye calibration model, One-Euro filter, drift estimator | 39deb2b, 274b76c |
| 2026-10-14 ~16:00 | desk mode: hands and eyes not visible in the headset | hands drawn at lower lens rim (fixed offset); gaze pointer not shown | placement fix (pimax-hands rest placement), analysis hands-desk.md; gaze over controllers; still open | c958134, aadb3ec, 7d0ab4a (wayvr), eff9cd8 |
| 2026-10-14 08:00-09:00 | passthrough offline proof only | not yet live | Rust OpenXR passthrough client; 25 fps, about 41 ms, timewarp compensates IMU rotation | f1c561d, 8fc3cdb |
| 2026-10-14 08:00-09:00 | GPU jitter in desk | no realtime queue | `setcap cap_sys_nice+ep` on monado-service (p99 GPU 4.9 to 2.7 ms) | 2500694 |
| 2026-10-14 08:00-09:00 | Mercury hand tracking fails | `--source any` bug in xr-handtest | fixed; live PASS when worn (about 30 % valid per hand) | 4764d32, 367939d |
| 2026-10-14 08:00-09:00 | DP audio silent | card profile | `output:hdmi-stereo-extra1`, unmuted, 100 % | n/a (runbook) |
| 2026-10-14 08:00-09:00 | camera self-calibration dy 3-8 px in controller frames | overexposed frames | selfcal merged; more lit recording owed | 3af8d2b, 5fe681c |
| 2026-10-14 afternoon | 6DoF: no translation (rotation only) | 3DoF driver | Basalt SLAM behind PIMAX_SLAM=1; phase 1 offline | cb9ff16 (Monado slam), a80cd2a, 60b479c |
| 2026-10-14 late | 6DoF live walk: AE stall (tracking stops) | Basalt queue deadlock after the sensor re-init gap | gate the IMU fed to the tracker | Monado d3138d4, analysis e03d50b |
| 2026-10-14 late | camera auto-exposure would need 0x51 | approved 2026-10-14; first push deadlocked host queue | AE controller in code, never sends unless `PIMAX_CAM_AE=1` and approved | 7a9f230, c0a1879 |
| 2026-10-14 afternoon | voice: no text | Dream Air mic sends zeros while the panel is off | `no_device` status; energy-gated Silero VAD; 0.75 s end-of-speech | 97d3891, e25950f, 845097f |
| 2026-10-14 afternoon | IPD: lens IPD and eye-measured IPD differ | measured IPD 65.88 live vs 65.93 Pimax dump; motor set needed | IPD measure and approved set-IPD motor command (`58 06 5c 05 01 lo hi`) with verify; watch view | 07528d6, 5d0e4d9 |
| 2026-10-14 afternoon | desk mode controller rays straight ahead | no aim | `PIMAX_CTRL_AIM=head` by default in desk | eff9cd8 |
| 2026-10-13 21:50 | NVIDIA PR #1262 comment | not posted | draft only (user to review) | 0513e2e (draft) |

---

## 5. Open issues and next steps

### 5.1 Open issues (from STATUS.md, HISTORY.md, NEXT-STEPS.md)

| # | issue | state | what is needed |
|---|---|---|---|
| 1 | 6DoF head tracking | phase 1 offline; phase 2 (AE, smoothing, room memory, 4 cameras) in progress; 6DoF not live; drift not measured live | approve camera exposure (0x51), run the walk recording, live test |
| 2 | Camera exposure command 0x51 | Approved by the user 2026-10-14 (Windows ranges only). The code never sends it unless `PIMAX_CAM_AE=1` | user approval |
| 3 | Eye accuracy | 8.8 deg raw, offset about 10 deg, gain about 0.6; goal about 1 deg | run pimax-eyecal worn; device calibration blob stays off until approved |
| 4 | Eye chip wedge (0x547) | recovery code exists (USBDEVFS_RESET after 2 failures) but untested live | live test with the lock |
| 5 | Desk mode hands and gaze not visible | WayVR gaze pointer and hands exist; user saw neither; investigation | live check after the placement fix |
| 6 | Freeze proof | fix works in 60 s and 300 s worn runs; `merged_head_sync=0` A/B not done; root cause "most likely" | run the proof with the fix off, under the rules |
| 7 | Long game session | not done | play a long session |
| 8 | Lens LUT | exact LUT looked worse; fit kept | re-judge in games |
| 9 | 90 Hz | new: the start commands (`1a 05 [02 5a]`, `1a 05 [05]`, keepalive `39 00 5a`) match the Windows capture 2026-10-15 take 6 byte for byte (pimax-proto test); app shows "90 Hz (new)" | user confirms the picture in the headset |
| 10 | Voice-to-text (EN + ES) | experimental; mic zeros when panel off; not tested in headset | `omarchyvr-voice listen` live test, then `desk start --voice` |
| 11 | Optical controller tracking | not started | research |
| 12 | Vocabulary `voc_GEN_8X4.dbow` | not copied from Windows | copy from Pimax runtime folder |
| 13 | NVIDIA PR #1262 comment | draft, not posted | user review |
| 14 | Monado slam build has no cap_sys_nice | getcap empty | setcap (root) when testing 6DoF live |
| 15 | Eye module auto-enable | built; "being finalised" in STATUS | confirm with a power cycle |
| 16 | Portability | NVIDIA 20/30/40 likely; 50 series unknown; AMD plausible; Intel unlikely | `research/portability-and-calibration-gui.md` |
| 17 | Pacman hook live test | hooks installed; real upgrade not yet seen | verify on the next nvidia-open-dkms upgrade |
| 18 | Docs drift | STATUS says hooks pending; they are installed on this machine (checked) | update STATUS.md |

### 5.2 Ordered next steps (NEXT-STEPS.md)

1. Install the pacman hooks (root): done on this machine, verify on next upgrade.
2. Approve the camera exposure command so 6DoF phase 2 can run live.
3. 6DoF recording walk, then a live 6DoF test.
4. Eye precision toward 1 degree, then the new calibration app.
5. Hands and gaze visible in desk mode (WayVR).
6. Freeze proof run with `merged_head_sync=0`.
7. Long game session; re-judge the exact LUT in games.
8. Voice-to-text test (EN + ES).
9. Later: optical controller tracking, 90 Hz test, copy `voc_GEN_8X4.dbow`.

### 5.3 Items a GUI should expose

- Status tiles: headset on USB, keepalive active, eye chip present, NVIDIA patches applied (`--check`), merged_head_sync value, setcap per service binary, Xid in boot log.
- Actions with confirmation: VR start (fps 72/90), VR stop (`--grace 30`), desk start (--eye, --voice), reset-headset (exit or dp).
- Root steps shown as copyable commands (see 3.10).

---

## 6. Safety rules a GUI must respect

Source: HISTORY.md "Hard-won rules" and STATUS.md. A GUI must enforce these, not only document them.

### 6.1 Headset protocol (HID)

- Send only bytes seen in Windows captures. No invented commands.
- Approved commands and their limits:

| command | bytes | where | status |
|---|---|---|---|
| Wake / PC alive | `58 06 1a 05 01 00 00` | IF0 | used by keepalive |
| Keepalive | `0x39 [RR 00]` every ~213 ms | IF0 | used by keepalive |
| Refresh announce | `0x1a/5 [02 48 ..]` (72 Hz; 90 Hz with PIMAX_FPS=90) | IF0 | used by vr start |
| Stop | `0x1a/5 [04]` | IF0 | used by reset-headset (exit mode) |
| Restart headset (eSpiCmdSysReset) | `58 04 28 00 00`, reply `4c 04 28 00 01 07` | IF0 | approved 2026-10-15, once per request; `reset-headset` default, the app's Restart headset |
| Unpair controller | `58 03 05 SS` (SS 2 left, 3 right), reply `4c 04 05 SS 01 07` | IF0 | approved 2026-10-15, once per request |
| Controller vibration | `58 09 07 SS AMP DUR_lo DUR_hi 00 00 00` (no reply) | IF0 | approved 2026-10-15 (slot 2/3, amp 1-100 %, dur 1-65535 ms) |
| Eye module enable (power-on) | `58 05 3c 05 00 01` | IF0 | approved, once per enumeration, rate limited |
| IPD motor (set IPD) | `58 06 5c 05 01 lo hi` (mm x 100, clamped 58-72 mm) | IF0 | approved (confirmed by the Windows capture: `58 06 5c 05 01 a8 16` = 58.00 mm); `pimax-eye ipd --apply` / `--set MM` send ONCE; the app's Optics Apply uses `--set` |
| Eye calibration blob 0x456 and other eye device-side calibration ops (0x3f2, 0x407, 0x42e, 0x3fc, 0x44c) | | | NOT approved; refused in code; the blob once stopped the chip |
| Eye module disable | never captured | | never send |
| Camera exposure 0x51 (AE) | | | approved 2026-10-14; sent only with `PIMAX_CAM_AE=1` |
| Activation blobs 0x55/0x56/0x5b | | | not needed; never send |
| Firmware | | | never flash; read-only analysis only |

- Only one process may claim the headset: the lock `/tmp/omarchyvr-headset.lock` (`flock -w 3600`).
- Keepalive: IF1-IF3 hidraw must stay open and drained, or the headset detaches every ~5.4 s.
- Do not run the headset while the eye chip is claimed by a stuck tool.

### 6.2 Display and NVIDIA

- NEVER force a DRM hotplug (`echo off/detect > /sys/class/drm/.../status`). It desyncs Hyprland and wedges DP-1. Forbidden also as a "recovery".
- Keep DP-1 non-desktop (the EDID). Do not put it into the Hyprland desktop. Do not touch Hyprland or `monitors.lua` unasked.
- NEVER SIGKILL Monado, and never while a flip may be stuck. SIGINT first, then wait: `omarchyvr vr stop --grace 30`, `omarchyvr desk stop` (default grace 30). Kill by PID only, never by name pattern. The lens test script waits 15 s before SIGKILL.
- After any Xid (for example 31) or "Idling display engine timed out": reboot before any further headset test. A SIGKILL during a stuck flip wedges the display until reboot.
- Xid preflight: `omarchyvr vr start` refuses to start if this boot's kernel log has an NVIDIA Xid or "Idling display engine" (preflight.rs). `omarchyvr vr stop` checks the log after stopping.
- Do not SIGINT WayVR while other agents run; use the screencopy capture config (`capture_method: screencopy`) so WayVR does not use dmabuf capture at exit.
- After every `monado-service` relink: `sudo setcap cap_sys_nice+ep <path>` (GPU priority). Check with `getcap`.
- Re-apply the NVIDIA patches after every nvidia-open-dkms update; keep `merged_head_sync=1`. Check: `cat /sys/module/nvidia_modeset/parameters/merged_head_sync` prints 1.
- `omarchyvr vr reset-headset` does not clear a stuck NVIDIA display. Only a PC reboot does.

### 6.3 Install and system

- Root steps are shown and confirmed one by one (3.10). No auto-elevating commands: passwordless sudo exists on this machine, and a `limine-mkinitcpio` run once happened by accident.
- Do not install system files silently (EDID, modprobe, mkinitcpio, dkms, hooks, udev).
- Mount the shared NTFS drive by UUID `<shared_drive_uuid>` (`shared_drive_uuid` in `~/.config/omarchyvr/paths.toml`) only. Never by device name.
- Do not push to the Monado origin (upstream freedesktop). Keep the repo private: it contains serials and factory calibration.
- Do not post to NVIDIA PR #1262 without the user's OK.
- Do not upload backups to a remote host without asking.
- Do not commit other agents' uncommitted work.

### 6.4 Eye and camera

- Eye: use `--skip-cal --skip-vendor-write` for replay. Calibration ops stay refused. `--with-cal` only with approval.
- Camera AE: dry run only (`omarchyvr-slam ae`) until approved.
- Camera and eye tools open live devices: check that no other process has them (`pimax-eye watch` and `pimax-eyecal` need WayVR or desk mode stopped).

### 6.5 Recovery

| symptom | action |
|---|---|
| headset detached every ~5 s | keepalive service running? `systemctl --user status omarchyvr-keepalived` |
| panel black, Monado ok | `omarchyvr vr status`; check `merged_head_sync`; do not hotplug |
| Xid or Idling in log | stop cleanly if possible; reboot before the next test |
| frozen display after SIGKILL | reboot |
| eye chip silent after a power cycle | `pimax-eye power-on` (under the lock) |
| eye chip wedged (0x547) | `pimax-eye live` auto recovery (USBDEVFS_RESET, once per 60 s); otherwise replug and power-on |
