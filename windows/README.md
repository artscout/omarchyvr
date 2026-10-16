# Pimax Dream Air: Windows collection run

One run on the dual-boot Windows collects everything we can only get from the Pimax software:

- Pimax's own lens distortion (so the Linux fish-eye bulge can be fixed), FOV, eye poses (canting, IPD), hidden-area mesh.
- A pose log (head, controllers, input, gaze) as ground truth for our own SLAM.
- Pimax's data files (SLAM config, maps, calibration, profiles) and a list of the Pimax and Tobii binaries (names, versions, SHA-256).

The tool is **read-only towards the headset**: it only calls PVR "get" functions. No setter, recenter or haptic function is even bound in the code.

## 0. Prepare (on Linux)

The Linux root disk is LUKS-encrypted, Windows cannot read it. Copy the whole `windows/` folder to a drive Windows can read
(USB stick, exFAT/NTFS partition, ...). Pick a drive with a few GB free (the USB recordings in section 3 are big).

## 1. Run the dump (about 5 minutes + the pose logger)

1. Boot Windows.
2. Start **Pimax Play**. Connect the headset and **put it on** (or make sure Pimax Play shows it as connected and active).
   Leave Pimax Play running and SteamVR closed.
3. Double-click `pimax-dump-distortion.cmd` (in the copied `windows` folder).
4. A menu asks what to collect. Press **Enter** for everything, or type numbers, for example `1 3`:
   - `1` distortion + FOV + eye poses + hidden-area mesh (about 10 s)
   - `3` other info: IPD, device properties, camera calibration, config keys (about 5 s)
   - `4` copy Pimax data files + binary inventory (a few minutes, no headset needed)
   - `2` pose logger, 120 s by default. It asks you to press Enter first and counts down 5 s. See section 3 if you also record USB.
5. When it says Done, copy the new folder `pimax-windows-dump-<date>` (next to the script) back to Linux. Everything is in it:

| file | content |
|---|---|
| `pimax-distortion-<serial>.json` | the distortion dump, input of `pimax-lens fit-pvr` |
| `pimax-info.json` | IPD, properties, VST camera intrinsics/extrinsics if exposed, config keys that are set, one pose/gaze snapshot |
| `pimax-poses.csv`, `pimax-poses-meta.json` | pose log and its clock anchors (QueryPerformanceCounter + UTC) |
| `pimax-file-inventory.csv` | path, size, versions, SHA-256 of every dll/exe/sys under Pimax and Tobii |
| `files\` + `pimax-copy-log.csv` | copied data files (no dll/exe/sys), and what was skipped and why |
| `system-info.txt`, `pimax-dump.log` | environment, USB devices, full log |

If something fails the window stays open and the log says why. Typical causes: Pimax Play not running, headset not connected,
`libPVRClient64.dll` not found (then run the `.cmd` again with `-DllPath "C:\...\libPVRClient64.dll"` after finding it with
`dir "C:\Program Files\Pimax" /s /b | findstr /i pvrclient`). A failed part never stops the other parts.

Options (append to the `.cmd`): `-Parts "1 3"`, `-Seconds 180`, `-GridN 129`, `-LineN 1024`, `-NoPause`.

## 2. On Linux: fit the distortion

```
cd ~/pimax-re/rust
cargo run --release -p pimax-lens -- fit-pvr /path/to/pimax-windows-dump-.../pimax-distortion-<serial>.json
```

It prints the fit per eye and channel (residual in degrees per radius, max error), whether the curve is radially symmetric,
a comparison with the current `~/.config/omarchyvr/lens.conf` (angle at the same panel radius, radius needed for 5..50 degrees),
and writes the variant `~/.config/omarchyvr/lens-ipd/pvr.conf` (`cant_deg` from the eye poses). `lens.conf` itself is never touched.
Try it with `PIMAX_LENS_CONF=~/.config/omarchyvr/lens-ipd/pvr.conf`. Useful flags: `--dry-run`, `--per-channel`, `--v-down`,
`--terms N`, `--conf FILE`, `--out FILE`.

## 3. Recording guide (USB traffic and SLAM ground truth)

These go beyond the dump. USBPcap and Wireshark are already installed from the earlier session (USBPcap 1.5.4, needs the reboot that
loads its driver, already done if `C:\Program Files\USBPcap\USBPcapCMD.exe` exists).
Use `usbpcap-capture.cmd` in this folder (it asks for Administrator): it records **all** USB root hubs, full packets (snaplen 65535),
into `usb-captures\<time>-<name>-hub<N>.pcap`, and prints start/stop time in UTC. `-Seconds 0` (default) records until you press Enter.
Hub numbers: the camera (34A4:0014) and the headset (34A4:0044) sit on the same hub in the earlier captures (hub 2); the others are almost empty.

Disk space and sizes: the cameras stream about 100 MB/s of isochronous video. USBPcap keeps only the first ~64 KB of each ~2 MB camera
URB (about one per frame, 50 fps), so a recording grows by roughly 3-10 MB/s on the camera hub: **about 0.5-1.5 GB for 2 minutes**, and
a 90 s cold start can reach a similar size. Have **at least 5 GB free** on the target drive, and move files off the stick between recordings.
Complete camera frames cannot be rebuilt from these files (that is a USBPcap limit); the start of every frame holds the frame counter and
timestamp rows, which is what the alignment needs. HID/control/interrupt traffic (eye tracker, IMU, commands) is stored whole.

### (a) Eye tracker and cold start: three recordings

Always: Pimax Play **closed** at the start, headset **unplugged** (no power) at the start of the recording.

1. `cold1`: start `usbpcap-capture.cmd -Name cold1 -Seconds 120`, then plug in / power on the headset, wait until it is detected, start
   Pimax Play, wait until the picture is in the headset and eye tracking reports ready. This captures the eye tracker init including the
   16-byte authentication exchange (packets 0x76c / 0x777). Expect 0.3-1.5 GB.
2. `cold2`: close Pimax Play, unplug the headset, then repeat exactly the same steps with `-Name cold2`. Comparing the two shows whether the
   authentication response repeats. Same size.
3. `eyecal`: with Pimax Play running and the headset on, start `usbpcap-capture.cmd -Name eyecal -Seconds 150`, then run the eye-tracking
   calibration in Pimax Play from start to finish. Expect 0.5-1.5 GB.

Write down the wall-clock time you did each step (or just rely on the UTC start printed by the script).

### (b) SLAM ground truth: pose log + camera/IMU USB traffic at the same time

1. Pimax Play running, headset on and tracking. Make sure you have room to walk (clear floor, 2 minutes).
2. Start `usbpcap-capture.cmd -Name slam -Seconds 0` (it records until Enter). Wait for "RECORDING".
3. Start `pimax-dump-distortion.cmd -Parts 2 -Seconds 120` (the pose logger only). Press Enter when asked, wait for the 5 s countdown and the beep.
4. At the beep: **give the headset one sharp shake or tap on the desk** (a clear spike in the IMU and in the pose log, an independent time
   marker), then put it on (or keep it on) and walk slowly around the room for the 2 minutes: look around, turn, crouch a little, go near
   and far from walls, no fast jerks. Optional: hold both controllers in view for a part of it.
5. After the second beep ("POSE LOGGER DONE") press Enter in the USBPcap window to stop. Copy `usb-captures\*slam*.pcap` and the dump folder.

Time alignment on Linux: `pimax-poses-meta.json` gives `start/end_qpc` + `qpc_frequency_hz` and `start/end_utc_filetime`; every CSV row has
`t_qpc` and `utc_filetime` (100 ns ticks since 1601, UTC). pcap packet timestamps are system time, so the UTC FILETIME columns line up
with them directly; the shake gives a second check. `pvr_time` / `<dev>_time` are Pimax's own clock for the pose.

## What the tool does and does not do

- Uses `libPVRClient64.dll` of Pimax Play via `getPvrInterface(1, 32)` (the same interface CustomHeadsetOpenVR uses). Details and sources:
  `analysis/research/pimax-pvr-distortion.md`.
- Calls only: initialise, createHmd, getHmdInfo/Status, getEyeDisplayInfo, getEyeRenderInfo, getFovTextureSize, getHmdDistortedUV,
  getEyeHiddenAreaMesh2, getTrackingState, getInputState, getEyeTrackingInfo, get*Config (unknown keys just come back as "unset"),
  getTrackedDevice*Property, getTracker*, getVST*, getConnectedDevices, getTrackingOriginType, getTimeSeconds/getVersionString, and
  destroyHmd/shutdown at the end. Nothing is written to the headset or to Pimax's settings.
- The copy skips `.dll/.exe/.sys` and files whose path suggests credentials (`token`, `cookie`, `password`, `login`, `toolkit`, ...).
  Look into `pimax-copy-log.csv` before sharing the folder.
- PowerShell 5.1 with inline C# (compiled by Windows itself); no installs.
