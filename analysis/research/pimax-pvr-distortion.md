# Pimax PVR client API: how to read Pimax's own distortion (and what else it exposes)

Goal: dump the real Dream Air lens distortion from the Pimax runtime on Windows (`windows/pimax-dump-distortion.ps1`), then
fit our radial model (`pimax-lens fit-pvr`). Researched 2026-10-14 from public sources only (no Pimax binary was available).

## Sources

| what | URL |
|---|---|
| PVR C API headers (Pimax's PVR SDK, public mirror used by CustomHeadsetOpenVR as a submodule) | https://github.com/mbucchia/PVR |
| `PVR_API.h` (C wrappers; DLL loading; every `pvr_*` function) | https://github.com/mbucchia/PVR/blob/main/PVR_API.h |
| `PVR_Interface.h` (the function-pointer table `pvrInterfaceV32`, DLL name, entry point) | https://github.com/mbucchia/PVR/blob/main/PVR_Interface.h |
| `PVR_Types.h` (all structs and enums) | https://github.com/mbucchia/PVR/blob/main/PVR_Types.h |
| `PVRVersion.h` (interface version 1.32) | https://github.com/mbucchia/PVR/blob/main/PVRVersion.h |
| CustomHeadsetOpenVR, "Pimax Builtin" distortion = `pvr_getHmdDistortedUV` | https://github.com/sboys3/CustomHeadsetOpenVR/blob/main/CustomHeadsetOpenVR/src/Distortion/PimaxDistortionProfile.cpp |
| CustomHeadsetOpenVR, session handling, hidden area mesh, IPD/canting | https://github.com/sboys3/CustomHeadsetOpenVR/blob/main/CustomHeadsetOpenVR/src/Helpers/PimaxCommon.cpp |
| CustomHeadsetOpenVR submodule list (PVR = mbucchia/PVR) | https://github.com/sboys3/CustomHeadsetOpenVR/blob/main/.gitmodules |
| Issue #57: owner says raw distortion can be dumped from pi_server (a way "has been developed", output "not nice to work with") | https://github.com/sboys3/CustomHeadsetOpenVR/issues/57 |
| Local notes | `analysis/customheadsetopenvr.md` section 2a |

## How a client gets a session (from `PVR_API.h`, `PVR_Interface.h`)

There is no import library to link against for the interface: the SDK's header-only wrapper does this:

1. `LoadLibraryA("libPVRClient64.dll")` (`PVRCLIENT_DLL_NAME`, 64-bit; `libPVRClient32.dll` for 32-bit). Plain name, so found through the normal
   DLL search path. **Where Pimax Play installs it is not documented in these sources.** Pimax Play's runtime lives in `C:\Program Files\Pimax\Runtime\`
   (our own PiService logs and MagicAttach.log show `C:\Program Files\Pimax\Runtime\LibMagicD3D1164.dll` and `...\dfudriver\`), so the tool searches
   there first, then `Program Files\Pimax` recursively, System32 and `PATH`. Unconfirmed until the first Windows run.
2. `GetProcAddress(dll, "getPvrInterface")`; call `getPvrInterface(PVR_MAJOR_VERSION = 1, PVR_MINOR_VERSION = 32)` -> `pvrInterface*`, a plain struct of
   C function pointers (`pvrInterfaceV32`, 93 slots; slot order checked by script against the header, e.g. 0 initialise, 2 createHmd, 5 getTimeSeconds,
   6 getHmdInfo, 8 getEyeRenderInfo, 13 getTrackingState, 23 getFovTextureSize, 24 getHmdDistortedUV, 53 getTrackedDevicePoseState, 65 getEyeTrackingInfo, 68 getEyeHiddenAreaMesh2).
   Errors: `pvr_dll_failed`, `pvr_dll_wrong`, `pvr_interface_not_found` when the DLL, the export or the version are missing.
3. `iface->initialise()` (= `pvr_initialise`), then `iface->createHmd(&hmd)` (= `pvr_createSession`; several sessions per process are allowed).
4. Poll `getHmdStatus(hmd, &pvrHmdStatus)` until `ServiceReady && HmdPresent && !ShouldQuit` (CustomHeadsetOpenVR does exactly this before reading anything,
   `PimaxCommon.cpp` `EnsurePvrSession`).
5. Query. At the end `iface->destroyHmd(hmd)` and `iface->shutdown()` (`pvr_destroySession`, `pvr_shutdown`).

Required runtime state: the Pimax service (`PiService`/`pi_server`, started by Pimax Play) running and the headset connected. CustomHeadsetOpenVR
additionally reads `getIntConfig("no_render")` = OpenPort enabled; the info is only needed for SLAM tracking via PVR, the distortion/FOV calls are
independent. CustomHeadsetOpenVR runs a reconnect thread because the session is lost whenever the headset is unplugged (`ShouldQuit`).

ABI notes for a P/Invoke-style caller (Win64 calling convention, all functions are plain C): `pvrFovPort` (16 B) is passed by hidden pointer;
`pvrVector2f` (8 B) is passed in an integer register (bit-pack the two floats into a `ulong` in C#); out-structs are written through pointers.
Sizes matter: `pvrEyeRenderInfo` is padded to 72 B (`PVR_ALIGNAS(8)`), `pvrPoseStatef` is 96 B, `pvrTrackingState` 288 B, `pvrEyeTrackingInfo` 40 B.
The tool therefore reads all out-structs from a large zeroed buffer.

## `pvr_getHmdDistortedUV`

```
pvrResult getHmdDistortedUV(pvrHmdHandle h, pvrEyeType eye, pvrVector2f uv, pvrVector2f outUV[3]);   // slot 24
```

- Input `uv`: panel position of the eye in 0..1. CustomHeadsetOpenVR (`PimaxDistortionProfile.cpp` lines 29-60) first undoes SteamVR's display rotation
  (`displayRotation` 3 = 270 deg for the Dream Air), the min-dimension square normalisation and the [-1,1] mapping, then calls it, so the input is the
  **native, unrotated panel UV of one eye** (0..1 over the eye's panel width and height).
- Output `outUV[0..2]` = R, G, B (index = colour channel): UV (0..1) in the **undistorted render texture** of that eye. They map this to SteamVR's
  `ComputeDistortion` result `(outUV - 0.5) * 2`. The texture spans the eye's FOV port from `getEyeRenderInfo` (they feed `-LeftTan, RightTan` and swapped
  `DownTan, -UpTan` into `GetProjectionRaw`, "Top and bottom are backwards per SteamVR documentation"). So: direction = **panel UV -> render UV**, per colour,
  tangent = linear in the render UV over the FOV port. Vertical direction of the output V (0 = bottom or top) is inferred from that flip (0 = bottom);
  `pimax-lens fit-pvr --v-down` flips it. Uncertain until compared with real data.
- It returns `pvrResult`; non-zero results are counted in the dump (`failed_calls`).
- Rotation/aspect handling in our converter: the 8 flips/rotations of the square and four panel-size conventions are tried and the best radial fit wins,
  so a wrong guess about the display rotation does not matter; only the V direction (above) is assumed.

## Everything else useful (all read-only, `PVR_API.h`)

| want | call (slot) | notes |
|---|---|---|
| per-eye FOV, eye offset/canting, pixels per tan | `getEyeRenderInfo(h, eye, pvrEyeRenderInfo*)` (8): `Fov{UpTan,DownTan,LeftTan,RightTan}`, `PixelsPerTanAngleAtCenter`, `HmdToEyePose{quat xyzw, pos}`, `DistortedViewport` | IPD = distance of the two `HmdToEyePose.Position`; canting = half the angle between the two orientations (CustomHeadsetOpenVR `PimaxCommon.cpp`, "Always query without parallel projection": they `setIntConfig("view_rotation_fix", 0)` first, **we do not set it**, we only read it, so a Pimax Play that has parallel projection enabled reports zero canting; the dump records the value) |
| recommended render texture size | `getFovTextureSize(h, eye, fov, pixelsPerDisplayPixel, pvrSizei*)` (23) | `GetRecommendedRenderTargetSize` rounds up to 4 |
| hidden-area mesh | `getEyeHiddenAreaMesh2(h, eye, type, pvrVector2f*, count)` (68); returns vertex count (call with null/0 first). Types: 0 hidden triangles, 1 visible triangles, 2 border line list, 3 visible rectangle. The C wrapper `pvr_getEyeHiddenAreaMesh` maps to this slot. | slot 59 `getEyeHiddenAreaMesh` is the older untyped one |
| HMD identity | `getHmdInfo` (6): product, manufacturer, vendor/product id, serial (24 chars), firmware major/minor, resolution | |
| display | `getEyeDisplayInfo(h, eye, pvrDisplayInfo*)` (7): EDID vid/pid, direct/extend, `eye_rotate` (x90 deg), `eye_display` (side by side / full), width, height, pos, refresh rate | |
| status | `getHmdStatus` (9) | |
| head + controller poses | `getTrackingState(h, absTime, pvrTrackingState*)` (13): head + 2 hands as `pvrPoseStatef` (pose, angular/linear velocity, angular/linear acceleration, time, status flags); `getTrackedDevicePoseState(h, deviceType, absTime, ...)` (53) for a single device incl. trackers; `getTrackingStateByPid` (91) | time source: `getTimeSeconds` (5). No raw IMU is exposed, velocities/accelerations are the closest. |
| buttons, sticks, triggers | `getInputState` (15) | |
| eye tracking gaze | `getEyeTrackingInfo(h, absTime, pvrEyeTrackingInfo*)` (65): `GazeTan[2]`, time, convergence distance, blink[2] | used by CustomHeadsetOpenVR `EyeTrackingThread` |
| tracker cameras | `getTrackerCount/Desc/Pose` (44-46) | |
| video see-through | `getVSTType/StreamFormat/CameraDistortionParams/CameraIntrinsics/CameraExtrinsics` (69-73) | intrinsics + fisheye4 distortion + camera-to-HMD pose per camera (potentially useful for the passthrough work) |
| device properties | `getTrackedDevice{Int,Float,String,Int64}Property(h, device, prop, def)` (54-56, 64) | props incl. product/serial/firmware strings, FPGA/VRC/radio versions, `IpdRangeMin/Max`, `SecondsFromVsyncToPhotons`, `SupportsEyeTracking`, `HmdTrackingStyle` |
| settings | `getInt/Float/String/Int64Config(h, key, def)` (18, 16, 20, 60) | known keys: `ipd`, `eye_height`, `no_render`, `hmd_codename`, `view_rotation_fix`, `hmd_buttons`, `enable_foveated_rendering`. The runtime has no key enumeration; the tool probes a list (unknown keys come back as the default and are reported as unset). Brightness/refresh-rate keys are guesses. |
| perf | `getPerfStats` (66) | not used |

**Never called** (setters/actions in the same table): `setTrackingOriginType`, `recenterTrackingOrigin`, `setFloat/Int/String/Vector3f/Quatf/Int64Config`,
`triggerHapticPulse`, `submitFrame/beginFrame/endFrame`, `logMessage`, swap chain functions. The tool has no delegate declared for them.

## Open questions to settle on the first Windows run

1. Location of `libPVRClient64.dll` (searched automatically, logged).
2. Whether the installed runtime accepts interface 1.32 (tries 1.32 down to 1.28 and warns when it is not 1.32).
3. Output V direction and the exact panel size convention (handled by the converter's search + `--v-down`).
4. Whether `getHmdDistortedUV` is a local computation (fast) or an RPC to pi_server (the dump of ~41 k calls would then take longer; the log prints the time).
5. Whether `view_rotation_fix` is 1 in the user's Pimax Play (then poses carry no canting and the distortion is relative to parallel views).
6. If the output looks non-radial: the dump contains the full 129x129 grid per channel; a mesh/LUT lens mode would be needed (not built).
