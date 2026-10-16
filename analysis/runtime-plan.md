# Runtime plan: Monado + xrizer on Dream Air (Omarchy, RTX 4090, NVIDIA 610.57.04)

Scope: research and local read-only inspection only. Nothing installed, no system settings, no Hyprland, no /dev/hidraw*, no sudo, no commits.
Confidence scale: H = verified by fetched source or local command, M = documented but not fetched in full, L = recollection or inference.

## 1. Local state (verified)
- `pacman -Qs` for monado|xrizer|opencomposite|envision|wivrn|openxr: no installed match (H).
- `extra/openxr 1.1.60-2` is in official repos. This is the Khronos loader only, not a runtime (H).
- `/usr/share/openxr` and `/etc/xdg/openxr`: absent. `monado-service`: not on PATH (H).
- AUR (via https://aur.archlinux.org/rpc/v5/search):
  - `monado` 25.1.0-3 (maintainer dbermond, 53 votes), plus `monado-doc` (H for name/version, M for currency).
  - `lib32-xrizer` 0.6-1 seen. Native `xrizer` package presumably exists but was truncated in the search (M).
  - `lib32-wivrn-server` seen, with `wivrn-server` as the native base (M).
- Repo `pimax-data`: no lens or distortion table in profile.json. The only hits are "Pimax K11" and eye_tracking_switch=0 (H). Lens data lives in analysis/customheadsetopenvr.md, not in the captures.

Sources: https://archlinux.org/packages/?q=openxr (not fetched; pacman data used), https://aur.archlinux.org/packages/monado, https://aur.archlinux.org/packages/xrizer

## 2. Monado driver architecture
- Official dev guide: https://monado.pages.freedesktop.org/monado/writing-driver.html (H for steps, L for file paths).
- Steps given by the guide:
  1. Copy the Sample HMD driver (drv__sample group) and rename identifiers (sed script in guide).
  2. One xrt_device per hardware piece. Use a `*_system` component if headset and controllers share one USB device.
  3. Detection: VID/PID with `xrt_prober_found_func_t` (the guide cites `hdk_found`), or an `xrt_auto_prober` for custom HID probing.
  4. Register entries in `src/xrt/targets/common/target_lists.c` (read at xrt_instance startup).
  5. Implement the xrt_device interface.
- Sample driver files (H, from https://monado.pages.freedesktop.org/monado/group__drv__sample.html): `sample_hmd.c`, `sample_interface.h`, `sample_prober.c`. Expected path: `src/xrt/drivers/sample/` (M).
- Probing: `xrt_prober` `select` walks drivers and auto-probers over the device list from `xrt_prober::probe` (H, guide).
- xrt_device callbacks (get_tracked_pose, get_view_pose, update_inputs, distortion): not verified. Check `src/xrt/include/xrt/xrt_device.h` in the Monado repo (L on exact names, M that these are the concepts).
- Simplest existing driver to copy for 3DoF IMU + display: start from `drivers/sample` (smallest, H). For a real HID IMU HMD, read `drivers/hdk` (IMU over HID; cited by guide), then `drivers/xreal_air` (glasses with IMU and no tracking camera, M), then `drivers/simula` (M). Vive/Survive are lighthouse-tracked (not a good template for a 3DoF IMU HMD). Rift S is inside-out with cameras (not a template).
- Repo: https://gitlab.freedesktop.org/monado/monado (paths above are relative to `src/xrt/`).
- Out-of-tree driver: possible in principle, but the target lists are compiled in. The practical path is an in-tree-style driver in a local Monado fork.

## 3. Direct mode on NVIDIA + Wayland/Hyprland
- Monado about page says direct mode works on Intel, AMD and Nvidia, and it is handled automatically like SteamVR (H, https://monado.freedesktop.org/about-runtimes.html via search result).
- Env vars (from https://monado.pages.freedesktop.org/monado/getting-started.html, H):
  - `XRT_COMPOSITOR_FORCE_XCB=1`: no direct mode, windowed compositor (useful for debugging without the headset).
  - `XRT_COMPOSITOR_XCB_FULLSCREEN=1`, applies only with FORCE_XCB.
  - `XRT_COMPOSITOR_FORCE_WAYLAND=1`: run compositor on Wayland (usually unneeded).
  - `XRT_COMPOSITOR_WAYLAND_CONNECTOR=DP-1`: pick the connector for Wayland direct mode.
  - `XRT_COMPOSITOR_FORCE_NVIDIA=1`, `XRT_COMPOSITOR_FORCE_RANDR=1`: force the init path.
  - `XRT_COMPOSITOR_FORCE_NVIDIA_DISPLAY=PNP`: direct mode on a GPU not in `NV_DIRECT_WHITELIST` (comp_settings.h).
  - `XRT_COMPOSITOR_PRINT_MODES=1`: list the HMD modes.
  - `XRT_COMPOSITOR_DESIRED_MODE=N`: pick a mode; default is highest resolution then highest refresh.
  - `XRT_COMPOSITOR_LOG=debug`, `XRT_LOG=debug`, `XRT_PRINT_OPTIONS=1`, `XRT_MESH_SIZE=64`, `XRT_COMPOSITOR_SCALE_PERCENTAGE=100` (default 140).
- Mode selection: Monado picks from the EDID-reported modes (`PRINT_MODES` lists them). I did not find documentation on a custom-mode override (L). Practical implication: if the EDID is broken or lacks the mode, Monado cannot choose it without a driver patch or `drm.edid_firmware`.
- Non-desktop: the Monado docs describe an xrandr `non-desktop` property step for AMD/Intel only, with the note that the kernel quirk list covers only the common HMDs (H). Whether NVIDIA needs the property is not documented in what I fetched (M).
- Wayland DRM lease: protocol `wp_drm_lease_device_v1` merged into wayland-protocols in 2021; GNOME Mutter support June 2024; XWayland 2021 (H, https://www.phoronix.com/news/Wayland-DRM-Lease-Merged, https://www.phoronix.com/news/XWayland-DRM-Leasing).
- Hyprland: I could not confirm whether Hyprland 0.56 offers `wp_drm_lease_device_v1` or a non-desktop/VR monitor option. The wiki fetch was redirected and then returned 404 (L on any claim here). Action: check `hyprctl`/Hyprland source for "drm_lease" and the monitor docs locally in a later step.
- NVIDIA caveats: Monado docs say proprietary NVIDIA is tested for Vulkan and OpenGL (H). I found no NVIDIA-specific lease bug list for 5xx/6xx (L). Our own finding: NVKMS and DSC are the blockers, not the lease protocol (see analysis/nvkms-mode-validation.md and analysis/dsc-research.md).

## 4. Lens distortion
Local data:
- No Pimax lens JSON exists in pimax-data (H, grep and profile.json inspection). Distortion tables exist only in the reference repo summarised in analysis/customheadsetopenvr.md:
  - "Dream Air Default" and "Dream Air like Pimax" RadialBezier tables (green, red, blue chromatic offsets), `DistortionProfileConstructor.cpp:112-208`. Eye-space angle in degrees mapped to radial percent of min(resX,resY).
  - Optical centre offset X -2.71 %, Y +0.9 %; canting 2 deg; display rotation 270 deg; panel 3840x3552 per eye; max FOV 98 x 87 or 88.
  - Colour matrices: `ShaderReplacement.cpp:372-405`. EDID vendor 53826 (0xD242 "PVR").
  - Source is GPL-2.0. Monado is BSL-1.0. Copying tables into Monado is a licence conflict, so re-measure or ask the author (H, from the existing analysis).
- Panel and lens fields we still need to measure ourselves: real FOV per eye, IPD (63 mm default, live value at info-report offset 45-46), and the real panel geometry.

Monado side (L on exact names, verify in repo):
- Distortion is modelled as a mesh: `u_distortion_mesh` helpers under `src/xrt/auxiliary/util/` with per-eye grids and a compute_distortion callback. Polynomial models ("poly_3k"-style) and OpenHMD-style coefficient models are also used by some drivers. Verify in `src/xrt/auxiliary/util/u_distortion_mesh.*` before coding.
- Simplest first step: use the Monado "no distortion" or simple-mesh path with a radial function fitted from the RadialBezier table, then refine.

## 5. Shortest path to "Monado test app, 3DoF head rotation in the headset"
Ordered steps. Steps 1-4 do not need DSC or the display to be lit.

1. Install Monado and xrizer later, not now. Check with the user first. Packages: `monado` 25.1.0-3 (AUR), `xrizer` (AUR, native name to confirm). Build a test OpenXR app against `extra/openxr`.
2. Run Monado with the simulated/qwerty-style no-HMD path on a desktop, not the headset: `XRT_COMPOSITOR_FORCE_XCB=1`. Goal: the OpenXR test app creates a session and renders. Monado's own drivers list does not show `simulated` or `qwerty` in what I fetched (M). Check `src/xrt/targets/common/target_lists.c` in the repo before relying on it.
3. Write the minimal `drivers/pimax` xrt_device driver (copy `drivers/sample`): HID open of VID 0x34A4 PID 0x0044 interface 2, parse the IMU stream (our IMU report `53 31 08 00`), publish a 3DoF orientation via get_tracked_pose. Test with `XRT_LOG=debug` and a dummy display (no compositor direct mode). This is the critical path that matches our existing RE. Keep `/dev/hidraw*` access to a read-only user with a udev rule (`linux/70-pimax.rules` is already in the repo, do not run it yet).
4. Add a fixed-FOV, no-distortion or simple-radial view config. Use a placeholder 2D window (FORCE_XCB) to verify head rotation moves the view.
5. Once DSC or a lit display works: enable direct mode with `XRT_COMPOSITOR_WAYLAND_CONNECTOR` or the NVIDIA force flags, select the mode with `XRT_COMPOSITOR_PRINT_MODES` and `XRT_COMPOSITOR_DESIRED_MODE`, and check non-desktop behaviour on Hyprland (step to verify locally).
6. Only after 5 works: distortion from the RadialBezier table (re-measured), IPD from the info-report offset, controllers, then xrizer for SteamVR games.

Smaller custom-mode test before DSC is fixed: if the EDID exposes a lower mode (e.g. a 1080p72 EDID already in analysis/dp-edid-1080p72.bin), we can test Monado direct mode with that mode via `XRT_COMPOSITOR_DESIRED_MODE`, without the full 4K panel. This is a smoke test for the lease path only, not a display-quality test. Confirm lighting and DSC state first (analysis/dsc-research.md).

## 6. Risks and unknowns
- Hyprland DRM lease support: unconfirmed (L).
- NVIDIA lease on 610.x: no evidence gathered (L).
- Monado distortion API names and file paths: not verified from source (M).
- Licence: customheadsetopenvr is GPL-2.0, Monado is BSL-1.0, xrizer is GPL-3.0. Keep Pimax-specific tables in a separate GPL-compatible plugin or re-measure.
- The Linux USB reconnect and EDID issues from the reference repo (issue #26) are still open risks.

## 7. Source list
- https://monado.pages.freedesktop.org/monado/writing-driver.html
- https://monado.pages.freedesktop.org/monado/group__drv__sample.html
- https://monado.pages.freedesktop.org/monado/getting-started.html
- https://monado.freedesktop.org/about-runtimes.html
- https://gitlab.freedesktop.org/monado/monado
- https://github.com/Supreeeme/xrizer
- https://aur.archlinux.org/packages/monado
- https://www.phoronix.com/news/Wayland-DRM-Lease-Merged
- https://www.phoronix.com/news/XWayland-DRM-Leasing
- https://gitlab.freedesktop.org/wayland/wayland-protocols/-/merge_requests/49
- https://github.com/sboys3/CustomHeadsetOpenVR (via analysis/customheadsetopenvr.md)
