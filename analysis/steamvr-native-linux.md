# Native SteamVR on Linux for the Dream Air (SLAM): architecture and phase 1

Date: 2026-10-14. Fork: `~/src/CustomHeadsetOpenVR` (clone of github.com/sboys3/CustomHeadsetOpenVR, upstream HEAD 4f25826, GPL-2.0), branch `linux-dreamair`, commit f78dda8. **Local only**: the fork's push URL is set to `DISABLED-local-fork-never-push`. Never push it, never open PRs or issues.
Earlier evaluation: `analysis/customheadsetopenvr.md`.

## 1. What already exists (facts checked today)

- **CustomHeadsetOpenVR (CHOVR) already builds on Linux, unchanged.** It has a `CMakeLists.txt` with the hidapi linux backend. All PVR code sits behind `#ifdef PVR_EXISTS`, which Linux never defines. The D3D shader replacement and the compositor plugin are stubbed on Linux (`ShaderReplacement::Initialize(){}`, `InjectCompositorPlugin` returns false). minhook is used only in `AAPVRBlocker.cpp`, behind `_WIN32`. Interface hooking uses `vtablehook.h`, which has a Linux `mprotect` path. So nothing Windows-only has to be stubbed for the build; only the behaviour has to be fenced off (see 4).
- **CHOVR can add its own HMD, not just shim one.** `FakeHeadset` uses `ShimTrackedDeviceDriver(impl, nullptr)`, and `BaseHeadsetShim` then serves the whole `IVRDisplayComponent`: window bounds, eye viewports, `GetProjectionRaw`, `ComputeDistortion` (RadialBezier per colour channel), render size, EDID vendor/product properties, IPD and canting (`SetIPD` with `eyeRotation`), the hidden area, and DSC properties 2110-2112.
- **CHOVR's Dream Air config fits our panel.** It uses 3840x3552 per eye, `displayRotation=3`, so the window is 7104x3840, the same as our EDID mode. It also has `eyeRotation=2` (our `cant_deg` 2.0), EDID vendor 53826 = 0xD242 "PVR" (our EDID: vendor PVR, product 4121 = 0x1019, from `linux/pimax-dp-edid-msvsdb.bin`), and an eye-tracking output that creates SteamVR's `/eyetracking` component (OpenVR 2.x `CreateEyeTrackingComponent`, `Prop_SupportsXrEyeGazeInteraction_Bool`).
- **Monado's SteamVR plugin is already built from our `desk` branch.** `~/src/monado-eye/build/steamvr-monado/bin/linux64/driver_monado.so` (16:55, after b0a4bca) includes our pimax prober ("Pimax Dream Air"), because `XRT_FEATURE_STEAMVR_PLUGIN=ON`. The plugin (`state_trackers/steamvr_drv/ovrd_driver.cpp`) runs the whole Monado driver stack inside vrserver (`target_instance_no_comp`). That covers our IMU fusion, Basalt 6DoF, the controllers and the pimax-lens distortion through `xdev->compute_distortion`. It has these gaps:
  - it sets **no** `Prop_EdidVendorID/ProductID`, which vrcompositor needs to find the direct-mode display;
  - `XRT_DEVICE_TOUCH_CONTROLLER` (what `pimax_ctrl.c` reports) has **no input bindings** (`case XRT_DEVICE_TOUCH_CONTROLLER: break; // TODO`), so the controllers would be tracked but have no buttons;
  - it has no eye-gaze component, and the model, serial and window bounds are generic.
- **SteamVR's Linux compositor supports our display path.** It is installed (build 1789498647), and `vrcompositor-launcher` has `cap_sys_nice=eip`. Its strings show `wp_drm_lease_device_v1/_request_v1/_connector_v1` ("No support for wl_drm_lease. This is needed for VR under Wayland"), `VK_EXT_acquire_drm_display`, `VK_EXT_acquire_xlib_display` and `Prop_EdidVendorID_Int32`. So under Hyprland it leases the non-desktop DP-1 through the same Wayland DRM-lease protocol Monado uses today, and picks the connector by EDID.
- **pimax-capi alone gives no pose.** It is the HID transport, decoders, video prepare, controller attach, the eye session and the hands. Fusion and SLAM live in Monado's C driver and Basalt. The Rust head-pose supervisor in pimax-cam needs Basalt's poses as input.
- **The OpenVR runtime is currently xrizer** (`~/.config/openvr/openvrpaths.vrpath`). `analysis/tools/vr_runtime_switch.sh steamvr` restores SteamVR.

## 2. Decision: (C) hybrid, in two stages

**Tracking and input come from our Monado driver, loaded in-process by SteamVR as `driver_monado.so` (option B's half). Display, optics, config and eye output come from the CHOVR fork (option A's half), which shims Monado's "Pimax Dream Air" HMD.** Both are SteamVR drivers in the same vrserver. No IPC is involved, and no second process owns the headset.

Why not the alternatives:
- **Pure (A), a standalone driver with pimax-capi**, means rewriting fusion, Basalt 6DoF, controller tracking and pose supervision in the OpenVR driver. That repeats months of verified Monado work, because pimax-capi only provides raw HID.
- **(A) with IPC to a running monado-service** is not possible in practice. monado-service owns the compositor and the DP-1 lease, and SteamVR's vrcompositor needs that same lease. Two processes would also fight over `/tmp/pimax-headset.lock`, the eye chip and the camera broker. A headless Monado (null compositor) plus an OpenXR or libmonado client inside the driver is the fallback if in-process loading fails, but it adds a whole IPC layer.
- **Pure (B), driver_monado.so alone**, cannot find the display (no EDID properties), has no controller buttons and no eye gaze. These can be fixed by patching ovrd (see 6). CHOVR already has the EDID, eye output, distortion profiles, hidden area and a JSON config that is re-read live. The hybrid uses CHOVR for those and keeps the Monado patch small: only the touch-controller bindings.

Stage 1 (built now) is a **standalone CHOVR HMD with a static stub pose**. It tests the SteamVR display path (lease, EDID match, mode, distortion orientation, canting) on its own, with zero tracking code and zero HID access. Stage 2 adds driver_monado.so, and the same class switches to shim mode on its own (`linuxDreamAir.mode = "auto"`).

## 3. Areas

**Display path and the old flip hang.**
- vrcompositor flips on the leased DP-1 through NVKMS like Monado does. The kernel-side fixes (`nvidia-2h1or-viewport`, `nvidia-2h1or-flip-fix` with `merged_head_sync=1`, `evo-makeroom-bounded-wait`) are in nvidia-modeset, so they apply to any lessee, SteamVR included. They were only proven with Monado's flip pattern.
- What SteamVR lacks is Monado's userspace **flipsafe** (self-revoke and `_exit` on a stuck flip). A stuck flip in vrcompositor will just hang.
- The same rules apply as for Monado: stop with SteamVR's own quit (the vrmonitor menu, or close the SteamVR window), never SIGKILL vrcompositor or vrserver, and reboot after any Xid or "Idling display engine".
- `pimax vr start`'s preflight (keepalived, DP-1 connected, Xid check) does **not** run when SteamVR starts from Steam. Run `pimax-doctor` by hand first.
- Unknowns:
  - whether vrcompositor drives the 2H1OR 7104x3840 mode cleanly;
  - whether CHOVR's manifest flag `"redirectsDisplay": true` keeps SteamVR from direct mode on Linux (Monado's manifest lacks it; if direct mode does not engage, test with it set to false in the installed copy);
  - which `IsDisplayOnDesktop/IsDisplayRealDisplay` answers vrcompositor wants. The class answers like Monado's ovrd: false/true.

**Distortion.**
- SteamVR samples `ComputeDistortion(eye, u, v)` into its own mesh, `distortionMeshResolution` 127.
- Stage 1 uses CHOVR's RadialBezier profile "Dream Air like Pimax": it was fitted to Pimax's compositor mesh, with centre -2.71/+0.9 % and CA tables.
- In stage 2, `linuxDreamAir.distortionSource = "device"` passes projection, distortion, render size and viewports through to Monado's `compute_distortion`, which is our pimax-lens LUT, the one verified worn. `"profile"` keeps CHOVR's. The best final form is to export our LUT as a CHOVR distortion profile JSON (Distortion/*.json), so it can be tuned live without restarting SteamVR.
- Caveat: CHOVR's `transformUV` rotation and eye-flip conventions for `displayRotation=3` were never checked on our unit. Check the first light with the lens align pattern.

**72/90 Hz.**
- The EDID offers 7104x3840 at 72 and 90. The driver sets `Prop_DisplayFrequency_Float` from `linuxDreamAir.displayFrequency` (72 by default).
- In shim mode the panel rate is chosen by Monado's `pimax_video_prepare(PIMAX_FPS)`, so set PIMAX_FPS to the same value.
- `DisplaySupportsRuntimeFramerateChange=false` (CHOVR's default), so there is no SteamVR rate switching. Upstream issues report SteamVR picking only 72 Hz on some setups.
- Next: `Prop_DisplayAvailableFrameRates_Float_Array`.

**Eye tracking.**
- SteamVR exposes eye gaze to OpenXR apps (`XR_EXT_eye_gaze_interaction`) through the driver's `/eyetracking` component. CHOVR's `EyeTrackingOutput` already writes it from per-eye angles and IPD.
- Stage 2 source: `pimax_eye_start/latest` from libpimax_capi (dlopen), with Monado's `PIMAX_EYE` left off so only one process opens the EyeChip. The alternative is to forward `XRT_INPUT_GENERIC_EYE_GAZE_POSE` from the Monado HMD.

**Controllers and hands.**
- driver_monado.so adds both pimax controllers with poses and an Oculus CV1 render model. Buttons need a small Monado patch: map the `XRT_INPUT_TOUCH_*` inputs to `/input/a|b|x|y|system|trigger|grip|joystick` in `ovrd_driver.cpp`, plus an input profile.
- Hand skeletons exist in ovrd only for Index controllers. They are optional (pimax-hands could feed `AddSkeletonControl` later).

**Windows-only parts.**
- PVR, AAPVRBlocker/minhook, the D3D11 shader replacement (colour matrix, subpixel, sharpening), the Windows config file watcher, and DisplayHelper's desktop lookup are all already compiled out or empty on Linux. The colour and subpixel features would need a Vulkan SPIR-V port inside vrcompositor and are out of scope.
- Fenced off by this branch: `PimaxCommon::TryDirectConnection` returns false under `CHOVR_NO_PIMAX_DIRECT_USB`. Upstream otherwise opens hidraw IF0/IF2 and can send `58 04 16` / `58 04 3A`, which would collide with keepalived and Monado.

**Licences.** CHOVR is GPL-2.0 and Monado is BSL-1.0. They are separate `.so` files in one process, and nothing is distributed, so there is no conflict. Do not copy CHOVR code into Monado.

## 4. Phase 1 slice (built)

Branch `linux-dreamair`, commit f78dda8:
- `src/Headsets/PimaxDreamAirLinux.{h,cpp}`: a `BaseHeadsetShim` subclass.
  - **standalone**: model "Pimax Dream Air (Linux)", tracking system `pimax_linux`, a static stub pose at `stubHeight` (1.6 m), `/input/system/click`, frequency from config, and all of BaseHeadset's display component with `dreamAir` config.
  - **shim**: wraps an HMD whose model contains "Pimax Dream Air". The poses stay Monado's.
- `Config.h` and `ConfigLoader.cpp`:
  - under `CHOVR_DREAMAIR_LINUX`, `dreamAir` defaults to `edidProductId=4121` and the profile "Dream Air like Pimax";
  - a new `settings.json` block `"linuxDreamAir": {"mode": "auto|shim|standalone|off", "standaloneDelay": 5, "displayFrequency": 72, "distortionSource": "profile|device", "stubHeight": 1.6}`.
- `DeviceProvider.cpp`: in auto mode, the standalone HMD is added after `standaloneDelay` s only if no other driver has added an HMD. The shim is attached to the first HMD that another driver adds.
- `CMakeLists.txt`: option `CHOVR_DREAMAIR_LINUX` (ON) defines `CHOVR_DREAMAIR_LINUX` and `CHOVR_NO_PIMAX_DIRECT_USB`.
- Verified:
  - `cmake` + `make` builds `CustomHeadsetOpenVR/bin/linux64/driver_CustomHeadsetOpenVR.so` with no errors;
  - `dlopen` works and `HmdDriverFactory("IServerTrackedDeviceProvider_004")` returns the provider (an unknown interface returns rc 105);
  - no unresolved libs.
- Not run: SteamVR, the headset, Monado.

The driver never touches USB. In standalone mode, our tools must wake the panel. In shim mode, Monado's pimax driver inside vrserver wakes it as it does today.

## 5. Commands for the later live test (user runs them; nothing here was run)

Build:
```
cd ~/src/CustomHeadsetOpenVR/CustomHeadsetOpenVR && cmake -S . -B build -DCMAKE_BUILD_TYPE=Release && cmake --build build -j16
# result: build/CustomHeadsetOpenVR/  (driver.vrdrivermanifest, resources/, bin/linux64/driver_CustomHeadsetOpenVR.so)
```
Preflight, every time:
```
pimax-doctor                                     # 13/13 OK, merged_head_sync=1, no Xid this boot
journalctl -k -b | grep -E 'Xid|Idling display engine'   # must be empty; otherwise reboot first
pimax vr stop --grace 30; pimax desk stop        # nothing else may hold DP-1, the lock or the eye chip
systemctl --user is-active pimax-keepalived      # must be active (wake + keepalive)
analysis/tools/vr_runtime_switch.sh steamvr      # OpenVR runtime back to SteamVR (undo: ... xrizer)
```
Stage 1 config (`~/.config/CustomHeadsetOpenVR/settings.json`):
```
{ "linuxDreamAir": {"mode": "standalone", "displayFrequency": 72},
  "dreamAir": {"proximitySensorType": 3, "stationaryDimming": {"enable": false}} }
```
Static pose: keep the display from dimming (proximity always on, no stationary dimming).

Register, stage 1:
```
VRP=~/.steam/steam/steamapps/common/SteamVR/bin/vrpathreg.sh
$VRP adddriver ~/src/CustomHeadsetOpenVR/CustomHeadsetOpenVR/build/CustomHeadsetOpenVR
$VRP show                                         # lists external drivers
pimax video-on 72 40                              # wake the panel with our existing tool (bytes it already sends)
```
Then start SteamVR from Steam and look at `~/.steam/steam/logs/vrserver.txt` and `vrcompositor.txt` for "PimaxDreamAirLinux", the EDID match and the lease. Expect a static, correctly distorted view.

Stage 2 (6DoF and controllers): also register Monado's plugin.
```
$VRP adddriver ~/src/monado-eye/build/steamvr-monado
```
Set `"mode": "auto"` (or `"shim"`), and `"distortionSource": "device"` to keep our pimax-lens LUT. Give vrserver the env our launcher sets: in SteamVR's Steam launch options use `PIMAX_FPS=72 PIMAX_SLAM=1 %command%`, plus the camera broker under the lock as in `pimax vr start --6dof`.

**Stop:** SteamVR's own quit only (vrmonitor menu > Exit, or close the SteamVR status window), then wait. Never `kill -9` vrserver or vrcompositor, and never kill by name. If the view freezes, SteamVR quit first. After any Xid, reboot before the next test.

Undo:
```
$VRP removedriver <same paths>; analysis/tools/vr_runtime_switch.sh xrizer
```

## 6. Risks

1. **Flip hang inside vrcompositor.** There is no flipsafe there. The kernel patches should cover it, but only Monado was verified. A hang means quit and wait, and reboot after an Xid.
2. **Direct mode may not engage.** Possible causes: the manifest's `redirectsDisplay`, the `IsDisplayOnDesktop/RealDisplay` answers, the EDID match on 0xD242/0x1019, or Hyprland's lease of a non-desktop output to vrcompositor. If it fails, SteamVR shows a desktop window and no harm is done.
3. **The panel stays dark in stage 1** if the `video-on` state does not outlive the tool. Pimax's display DRM ("display shuts off unless Pimax software stays connected") is handled on our side by keepalived.
4. **Distortion orientation.** `displayRotation=3` and the per-eye flips may be mirrored on our unit, and CA tables are per unit. Check with the align pattern before wearing it.
5. **Rate mismatch.** SteamVR's chosen mode must equal `PIMAX_FPS` and the panel's video rate, or we get judder or a black panel.
6. **In-process Monado.** A Monado crash takes down vrserver. Basalt and the camera broker inside vrserver are untested, and vrserver lacks `cap_sys_nice`.

## 7. Next steps

1. Stage 1 live test (user, with the lock and preflight). Note: lease OK?, mode, distortion orientation, canting.
2. Monado patch on `desk`: touch-controller input mapping in `ovrd_driver.cpp`, plus optional EDID properties and model/serial so pure (B) also works as the fallback.
3. Export the pimax-lens LUT as a CHOVR distortion profile, or keep `distortionSource=device`, and compare both with the align pattern.
4. Eye gaze: feed `EyeTrackingOutput` from `pimax_eye_*` (dlopen libpimax_capi) and check `XR_EXT_eye_gaze_interaction` through SteamVR's OpenXR runtime.
5. `Prop_DisplayAvailableFrameRates_Float_Array` with 72/90, and a hidden-area mesh from our lens fit.
