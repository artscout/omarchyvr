# Windows capture plan (next Windows visit)

Goal: get from Windows everything we still lack so Linux tracking and controllers can match Pimax Play, plus a side-by-side tracking
reference. Tool: `windows/capture-session.cmd` (double-click, Administrator prompt). It is **capture only**: USBPcap reads traffic,
nothing is written to the headset, controllers or Pimax settings. Pimax Play files copied are for local analysis, never redistributed.

## What the old takes were (conventions we keep)

- Scripts: `capture-all.ps1` (round 1, 2026-10-13 12:40-12:51), `capture-round2.ps1` (13:03-13:12), `windows/usbpcap-capture.ps1` (2026-10-14).
- USBPcapCMD from `C:\Program Files\USBPcap`, **one process per root hub, every hub**. Everything real is on **hub2**
  (headset 34A4:0044, bridge 34A4:0043, camera 34A4:0014, Tobii 2104:0561); hub1 is empty (24 B to 100 KB).
- Names: `YYYYMMDD-HHMMSS-<scenario>-hub<N>.pcap` (classic pcap, local time of the PC) plus `notes.txt` with
  `START name (snaplen N)`, `MARK name: text`, `STOP name`, local ISO time. Snaplen 512 for HID/settings takes (HID is 64 B, so whole),
  65535 for camera/tracking takes (USBPcap still cuts camera iso URBs at about 64 KB, enough for frame counter and timestamp rows).
- Where they went: `captures/`, `captures2/` (git-ignored), then `pimax-data/windows-20261014/usb-captures/` via the shared NTFS drive
  (UUID set as `shared_drive_uuid` in `~/.config/omarchyvr/paths.toml`, `omarchyvr mount-shared`). The new session writes `pimax-win-session-<date>\usb-captures\` plus `notes.txt`, `info\`,
  `pimax-play-logs\`, `pimax-play-install-files\` and one `...-poses\` folder per tracking take.
- Cold starts were done by stopping Pimax services (`PiServiceLauncher`, Tobii runtime) first. Scenario 2 does the same on request.

## Already captured / still missing / why

| Topic | Already captured (file) | Still missing | What the new take adds |
|---|---|---|---|
| Power on, init, camera/IMU start | 20261013-130457-cold-start, 20261014-043157-cold1, -043545-cold2 (hub2) | nothing about the sequence itself | Scenario 2 only if we want a take with `m plugged` / `m play` markers and full packets in one go; low priority, **skip if short of time** |
| Shutdown / headset off | 20261013-124051-shutdown, -125121-shutdown (short, 5 s) | power-off with headset switched off | Scenario 16: the real power-off sequence (backlight, audio amps, fan) |
| Controller buttons | 20261013-124629-controllers, 20261013-130931-controller-buttons (left hand only: trigger, grip, A, B, stick, menu) | right hand buttons; analog values | Scenario 5 covers the right hand and analog trigger/grip curves |
| Controller pair / unpair | none (the old takes only powered them on) | pairing traffic (`StartPair`, `PairRx`) | Scenario 3 |
| Bluetooth disconnect / reconnect | none | link loss and recovery per hand | Scenario 4 |
| Vibration (haptics) | none (analysis/controller-activation.md: no haptic pulse in any capture) | the haptic command, green-light moment | Scenario 5 with `m green now` / `m buzz now` markers, and scenario 15 (game rumble) |
| Brightness / IPD / refresh | 20261013-124541-ipd-brightness, 20261013-130710-settings (IPD 60 and 65.9, brightness 20 and 100, refresh 90 once) | clean 72 and 90 switch, brightness steps | Scenario 6: 72 -> 90 -> 72 with markers (`required fps` command, DP mode change) |
| Audio | only UAC GET_CUR at enumeration; amps enable in MCU log | any audio-enable or volume command, DP audio | Scenario 7 |
| Idle background | none clean | noise floor | Scenario 1 (30 s) |
| Tracking, long walk | 20261014-042409-slam (2.5 min, 645 MB) + pose log 76746 rows | still, slow-look, fast-turn, controllers in view | Scenarios 8-13: separate short takes, each with its own pose log, so every motion type can be compared with Linux |
| Eye tracking | 20261013-124906-eyetracking, 20261014-042748-eyecal | nothing | not repeated |
| Room setup / floor | none (chaperone files copied) | floor / play-area commands | Scenario 14 |
| Full game session | none | SteamVR path, game haptics, long load | Scenario 15, Dungeons of Eternity 3 min |
| Install files (controller models, JSON, calibration) | `pimax-data/` (`controller_params.json`, `device_calibration` xml, `platMap_orborb_K8L4.bin`) | `voc_GEN_8X4.dbow`, controller 3D model files, camera mode configs | Menu item F copies config/model/calibration files (no dll/exe/sys) |
| Logs | 6DOF_*.txt, p3c_mcu_*, Tobii logs from 2026-10-14 | logs of the new session | Menu items L (before and after) |

## Before (on Linux, 10 min)

1. Charge headset and both controllers. Copy `windows/` to the shared NTFS drive (`omarchyvr mount-shared`). Make sure 20 GB are free there
   (tracking takes with snaplen 65535 are 0.2-0.7 GB each; the whole session is about 6-8 GB).
2. Windows needs: **USBPcap** (already installed, 1.5.4). Wireshark is optional (only for `-Pcapng`). Nothing else.
3. Linux recordings for the same scenarios (see "Linux side" below) are made at **another time** on Linux with the same movements;
   write the time of each Windows take in the notes so we can compare.

## During (Windows, about 75-90 min)

Double-click `capture-session.cmd`, accept the Administrator prompt. It collects system info (monitors, sound devices, USB devices, dxdiag),
copies Pimax Play logs, then shows the menu: `A` runs all 16 scenarios in order, numbers run chosen ones, `F` copies install files.
Each scenario speaks and prints what to do, you press Enter, it counts 3-2-1 and beeps, you act, it stops (timed takes stop alone; Enter
stops earlier; `m text` + Enter is a marker). Tracking takes also start the Pimax pose logger (`pimax-dump-distortion.ps1 -Parts 2`).

| # | Scenario | Time | Why we need it |
|---|---|---|---|
| 1 | baseline-idle | 1 min | noise floor of the bus |
| 2 | poweron-home | 4 min | optional repeat of the init with markers |
| 3 | ctrl-pair-unpair | 5 min | pairing protocol |
| 4 | ctrl-bt-reconnect | 4 min | disconnect/reconnect behaviour |
| 5 | ctrl-buttons-haptics | 7 min | right hand input, analog values, vibration command |
| 6 | settings-bright-ipd-hz | 5 min | 72/90 Hz, brightness, IPD commands |
| 7 | audio-sound | 3 min | audio enable / volume path |
| 8 | track-desk-still | 1 min | IMU noise, bias, camera at rest |
| 9 | track-worn-still | 1 min | still gate, jitter |
| 10 | track-slow-look | 2 min | rotation reference |
| 11 | track-walk | 3 min | position reference |
| 12 | track-fast-turns | 2 min | exposure/rate under motion |
| 13 | track-controllers | 3 min | controller IR LED timing, loss/recovery |
| 14 | room-setup-floor | 5 min | floor commands |
| 15 | game-doe2 | 6 min | game load, game haptics, SteamVR |
| 16 | shutdown | 2 min | power-off sequence |

Totals: capture about 55 min, setup/menus/pauses about 25 min, info/log/install-file copy about 10 min. **About 90 min.** Short version
(1 hour): 3, 4, 5, 6, 7, 9, 11, 13, 15, 16 and F.

## After

Shut down Windows, boot Linux, `omarchyvr mount-shared`, move the session folder to `~/omarchyvr/pimax-data/windows-<date>/` (git-ignored).
Agents then: (1) decode haptic/pair/audio/refresh commands with `analysis/tools/usbp.py` and update `analysis/controller-activation.md`,
`init-sequence.md`, `windows-linux-command-diff.md`; (2) align pose logs (UTC FILETIME) with the pcaps and compare to Linux SLAM with
`omarchyvr-slam ate`/`corrections`; (3) read camera mode/exposure/frame rate from the camera hub traffic and from the new install files;
(4) extract controller LED model and sync from the install files; (5) add anything found to the Monado driver and rerun goldens.

## Linux side: matching recordings

Make these on Linux with the same movement (the app must offer them; another agent restores the Record card):

| Windows scenario | Linux recording | Tool |
|---|---|---|
| 8, 9, 10, 11, 12 | camera + IMU recording of the same motion | `pimax-cam record OUT.pxcam --seconds N` (IMU is embedded; also SLAM recording of the run for `omarchyvr-slam ate/corrections`) |
| 13 | controller IMU csv + optical tracking log while waving in/out of view | controller IMU csv from the controller recorder, plus `pxcam` for the LED frames |
| 3, 4, 5, 6, 7 | raw HID listening (no commands) of the same actions | `omarchyvr controllers --raw` style capture, usbmon/pimax-device log |
| 2, 16 | power-on and off log of our init | `omarchyvr` start/stop with the session log |
| 15 | what is seen in the headset (video) | `omarchyvr-record` (`omarchyvr record start`) |

**Record card needs:** (1) one "Scenario recording" button that starts camera+IMU, SLAM log and controller csv together and names them
`YYYYMMDD-HHMMSS-<scenario>` like the Windows files; (2) a scenario list matching the table above with spoken instructions in the headset;
(3) markers (button and voice "mark") written to a `notes.txt` in the same format; (4) video recording kept as is (eyes, fps, quality);
(5) a "Pose log" toggle that writes head and controller poses with a UTC clock anchor; (6) shows size, free disk, and the output folder;
(7) a stop that always finishes the files. Every control is a switch in the app, per the project rules.

## Redo session (menu R, after the 2026-10-15 capture; see analysis/windows-capture-20261015.md)
`capture-session.ps1` menu option **R** runs the takes that failed, in this order, each with its own text and markers:
3 pair/unpair per hand (put the controller in pairing mode first, then press Pair for that hand only, wait up to 60 s),
5 buttons + haptics (headset worn, both controllers), 13 controllers (headset worn, screen on, both controllers),
6 IPD only (brightness and 90/72 Hz are already captured), and last 2 = start recording, then **Restart headset in Pimax Play**
(the power button only turns the panels off and never the headset; never unplug). Take 4 (Bluetooth) is dropped:
the controllers talk to the headset radio. Take 16 stays "Play exit + panels off".
- Pre-check before 5 and 13 (and 15): both controllers connected (latest controller lines from the Pimax Play logs, then a spoken check + Enter; `w` wakes them and asks again, `s` starts anyway).
- Marker prompts at key moments (`m <text>`): untimed takes speak the next marker after each one; timed takes speak a marker moment at even intervals.
- Pose logger: flushes at least once per second and when stopped; the session ends it with `stop.flag` (killed only after 8 s); a failed PVR read (game start) no longer ends the log. The old CSVs were exact 1 MiB multiples and lost 2-4 s, the game take stopped after 28 s.
- Unpair seen in take 13: `58 03 05 03` (right). The redo take 3 captured the left one too (`58 03 05 02`, reply `4c 04 05 02 01 07`), so the app now unpairs both hands. Linux side: `omarchyvr controllers unpair left|right` (refused while Monado or SteamVR runs) and the Unpair button on each card.
- Scripts copied to the shared disk folder `New Volume/pimax-linux/windows/` too.
