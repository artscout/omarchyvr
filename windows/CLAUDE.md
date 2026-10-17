# Claude on Windows: collect a Dream Air's own Pimax data for omarchyvr

You are Claude Code running on the **Windows** side of a dual-boot PC. The Linux side runs omarchyvr (the Pimax Dream Air
on Linux). Your job: collect this headset's own calibration from Pimax Play, so Linux uses **this** headset's data
instead of the shipped Dream Air reference. Linux picks it up automatically from the shared drive.

This file sits in the `windows/` folder that Linux copied to the shared NTFS drive. Work only in that drive.

## Hard rules (read first)
- **Read-only towards the headset, the controllers and Pimax Play.** Use only the scripts in this folder: they call PVR
  "get" functions only. Never call a setter, recenter, pairing, haptics or firmware function, never change a Pimax Play
  setting, never stop or reinstall Pimax/Tobii services unless the user asks.
- **No firmware flashing. No registry edits. No admin prompt** unless a step below says so and the user agrees.
- **Nothing leaves this PC.** No uploads, no pastes, no web services. The files hold device serials: they stay on the
  shared drive. Linux scrubs serials before anything is ever shipped.
- Don't copy `.dll`, `.exe` or `.sys` files (the scripts already skip them).
- Talk to the user in plain words; they don't use the command line. Run the commands yourself.

## What exists on Windows, and what doesn't

| Linux file | On Windows? | Where / how |
|---|---|---|
| `headset-calibration.xml` | yes | Pimax's `device_calibration` XML under `%ProgramData%\Pimax` (found by step 2 copy) |
| `controllers/<hand>/controller_params.json` | yes | under `%ProgramData%\Pimax` (`...\slam\H1\<controller serial>\`), one per controller |
| `lens.conf` (lens distortion) | **not as a file** | Pimax computes it live: step 1 dumps it via the PVR API; Linux fits `lens.conf` from the dump |
| eye calibration | only inside Tobii's runtime | not collected here (Linux has its own eye calibration in the app) |
| `gyro-bias`, controller-camera tilt fit | **no** | measured on Linux; nothing to do here |

## Steps

1. **Prepare.** Pimax Play running, headset connected, switched on, and worn (or shown as active in Pimax Play).
   SteamVR closed. Both controllers on. Check the drive has 2 GB free.
2. **Dump + copy (about 5 minutes).** In this folder run:
   `powershell -ExecutionPolicy Bypass -File .\pimax-dump-distortion.ps1 -Parts '1,3,4' -NoPause`
   (1 = distortion, FOV, eye poses, hidden-area mesh; 3 = IPD, properties, camera calibration; 4 = copy Pimax data
   files + binary inventory). Skip part 2 (the 120 s pose logger) unless the user asks for it.
   If `libPVRClient64.dll` is not found, locate it under `C:\Program Files\Pimax` and rerun with `-DllPath "<path>"`.
3. **Check** the new folder `pimax-windows-dump-<date>`:
   - `pimax-distortion-*.json` exists and is not empty;
   - `files\` contains a `device_calibration*.xml` and at least two `controller_params.json`;
   - `pimax-dump.log` has no `FAILED` part (read the reason if it has, fix, rerun only that part).
4. **Place it for Linux.** Move the dump folder to the drive root and name it `omarchyvr-windows\` (replace an older
   one only after the user agrees). Part 4 already copied `%ProgramData%\Pimax` to `files\ProgramData-Pimax\`,
   the layout Linux's importer looks for.
5. **Tracking and stability configs (to inspire Linux, user request 2026-10-17).** Read (do not change) the copied
   data files in `files\` and make `omarchyvr-windows\TRACKING-CONFIGS.md`: one section per file that configures
   - the gyro/IMU: bias, noise, scale, temperature, sample rate, IMU-to-camera or IMU-to-head transforms;
   - stability: prediction time, smoothing/filter gains (Kalman, alpha-beta, one-euro), still/rest detection,
     jitter thresholds, relocalisation, recentre rules;
   - controllers: LED model/positions, camera intrinsics and stereo extrinsics (camera-to-camera, camera-to-head),
     exposure/LED blob settings, fusion of optical pose with the controller IMU, out-of-view behaviour;
   - SLAM (`slam\` folder, `*.yaml`/`*.json`/`*.xml`/`*.ini`/`*.txt`): feature counts, keyframes, map settings.
   For each: path relative to `files\`, format, and the meaningful keys with their values (a table), plus one line
   on what it probably does. Binary maps (`*.bin`, `*.dbow`): name and size only. Data files only: never open,
   decompile or disassemble `.dll`/`.exe`/`.sys`. Replace any serial number with `<serial>`.
6. **Write `omarchyvr-windows\REPORT.md`** for the Linux Claude: date, Pimax Play version, which parts ran,
   what was found (file names, NOT serial numbers), what failed and why, anything odd.
7. Tell the user: "Done. Boot Linux; the omarchyvr app picks up your headset's own calibration from the shared drive."

## What Linux does with it (for your report, you don't do this)
- The importer (`omarchyvr import-previous`, run by the app) reads `ProgramData-Pimax` on the mounted shared drive:
  headset calibration and controller models go to `~/.config/omarchyvr/`; the user's own data wins over the reference.
- The lens: `pimax-lens fit-pvr <pimax-distortion-*.json>` makes `lens.conf` (the Windows PVR fit).
- `TRACKING-CONFIGS.md`: Linux compares the gyro, filter and controller-stereo values with its own (Monado Pimax
  driver, `pimax-ctrltrack`, SLAM) and adopts what is better, as its own code and its own values.
