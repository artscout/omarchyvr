# Direct mode for Dream Air (Monado + Hyprland/NVIDIA)

Research and read-only inspection only. Nothing in system or Hyprland config was changed.

## 1. Local state of DP-1 (read-only)

- `/sys/class/drm/card1-DP-1/` has no `non_desktop` file. sysfs only exposes status, edid, modes, dpms, enabled, etc.
- `modetest -M nvidia-drm -c` (no master needed) shows connector 826, DP-1, `connected`, 120x70 mm.
  - Property **id 7 `non-desktop` exists, value = 0**. The kernel/NVKMS is NOT flagging the headset as non-desktop.
  - Modes include `7104x3840@72` (#2, phsync/pvsync) and `7104x3840@90` (#1), plus 3840x2160@72 preferred.
  - Property `DPMS` = 3 (Off).
- EDID header `00ffffffffffff00 42d2 1910`: manufacturer ID 0x42D2 decodes to **PVR**, product 0x1019. Monitor name descriptor is "Pimax".
- `/sys/kernel/debug/dri` is not readable by the user (permission denied), so no further kernel debug info.
- `drm_info` is not installed. `modetest` is at /usr/bin/modetest.

## 2. Kernel quirk table (drm_edid.c)

Source: https://raw.githubusercontent.com/torvalds/linux/master/drivers/gpu/drm/drm_edid.c (fetched via WebFetch; first 100k of 240k chars read; the quirk table itself is fully visible).

- `EDID_QUIRK('S','E','N',0x1019, BIT(EDID_QUIRK_NON_DESKTOP))` (Sensics VR)
- `EDID_QUIRK('S','V','R',0x1019, BIT(EDID_QUIRK_NON_DESKTOP))` (OSVR HDK/HDK2)
- **No `PVR` entry found.** Pimax is not in the upstream table as far as I could read.
- Proposed but unconfirmed patch: freedesktop drm/misc/kernel issue #40, "drm/edid: add non-desktop quirk to Pimax VR headsets" (PVR 0x1019/0x101a/0x101b). Author asked for confirmation before merging. Status unknown: https://gitlab.freedesktop.org/drm/misc/kernel/-/issues/40

**Confidence: high** that DP-1 is not non-desktop today (value 0 read directly). **High** that the kernel has no PVR quirk. Medium on whether the patch was merged since the cached page.

## 3. Hyprland DRM lease support

Sources: shallow clones of https://github.com/hyprwm/Hyprland and https://github.com/hyprwm/aquamarine (read locally, not built).

- Hyprland implements `wp_drm_lease_device_v1` (name "DRMLease"): `src/protocols/DRMLease.cpp` (365 lines), registered in `src/managers/ProtocolManager.cpp:243`.
- Offering: `src/output/Monitor.cpp:306-314`. Only when `m_output->nonDesktop` is true does Hyprland call `lease->offer(monitor)` for every lease device of the same backend. The monitor is then never configured or enabled (`return` at line 318).
- **Auto-offer:** yes, but only for connectors aquamarine marks non-desktop. Aquamarine reads the property at `aq/src/backend/drm/DRM.cpp:2077-2080`. Since DP-1 reads 0, nothing is offered today.
- **Disabled monitors:** the `monitorRule.m_disabled` branch (around Monitor.cpp:285-300) runs **before** the nonDesktop branch and returns, so a rule with `disabled = true` prevents the offer. For a leased headset, the monitors.lua entry must NOT be disabled. It should be present but not enabled for the desktop, which Hyprland handles as "not configured".
- **Config knob:** I found no option to force a connector to be leasable or non-desktop. The only path is the kernel flag (or aquamarine, which reads only the kernel property). Confidence: medium-high from code, since I did not grep the config parser for other keys.
- NVIDIA: no Hyprland-specific NVIDIA lease issues found in the search. NVIDIA's own driver forum thread reports that DRM lease acquisition fails on all display servers with proprietary modules: https://forums.developer.nvidia.com/t/nvidia-proprietary-non-open-modules-completely-unable-to-acquire-a-drm-lease-on-any-display-server-all-known-nvidia-drivers-any-hardware/341244 (the reported setup is not identical to ours; treat as a warning).

**Conclusion:** Hyprland can lease a DP-1 only if the kernel says non-desktop. Today it says 0, so Hyprland treats the headset as a normal desktop output. A quirk or patched kernel is the blocker.

## 4. Monado direct-mode path

Sources: Monado source (https://gitlab.freedesktop.org/monado/monado.git, shallow clone, read locally); direct-mode docs (https://monado.freedesktop.org/direct-mode.html, GitLab raw page was blocked by Anubis).

- `src/xrt/compositor/main/comp_window_direct_wayland.c`:
  - Requires `VK_EXT_acquire_drm_display` at compile time (`#error` at line 28 if missing); the extension is enabled at line 537-538.
  - Binds `wp_drm_lease_device_v1`, enumerates connectors (`_drm_lease_device_connector`, line 315), and selects one via `XRT_COMPOSITOR_WAYLAND_CONNECTOR` (line 31).
  - Calls `vkGetDrmDisplayEXT` with the leased fd and connector id (line 184).
  - Log lines to look for: `connector %s (%s) id: %d` (line 283), and a Wayland-direct selection message.
- `src/xrt/compositor/main/comp_settings.c`:
  - `XRT_COMPOSITOR_FORCE_WAYLAND_DIRECT` (line 24), `XRT_COMPOSITOR_FORCE_NVIDIA` (25), `XRT_COMPOSITOR_FORCE_NVIDIA_DISPLAY` (26), `XRT_COMPOSITOR_FORCE_VK_DISPLAY` (27), `XRT_COMPOSITOR_PRINT_MODES` (22), `XRT_COMPOSITOR_DESIRED_MODE` (32, default -1 = pick automatically).
- `comp_window_direct.c:120-131`: mode index from DESIRED_MODE; -1 means automatic choice.
- Caveat from docs: if the headset is also used as a desktop extended output, direct mode init fails.
- Third-party guide (Gaia Sky, 2026): sets `XRT_COMPOSITOR_FORCE_NVIDIA=0` and `XRT_COMPOSITOR_FORCE_WAYLAND_DIRECT=1`; success log `comp_window_direct_wayland_init Using wp_drm_lease_device_v1`; failure on NVIDIA logs `Selected NVIDIA Direct-Mode backend!` https://gaiasky.space/news/2026/gaiasky-vr-linux-monado

**Key blocker:** Wayland direct needs the compositor to *offer* the connector through drm-lease-v1. Hyprland will not offer it while the kernel says desktop. So the Wayland direct route is dead until non-desktop is set.

The XLIB direct path (`VK_EXT_acquire_xlib_display`) needs an X server with the display. On NVIDIA it fails with VK_ERROR_UNKNOWN per the forum thread. Confidence in that NVIDIA failure: low-medium (one report).

`XRT_COMPOSITOR_FORCE_VK_DISPLAY` (VK_KHR_display direct, no compositor, VT) requires the display to be unclaimed by any compositor. With Hyprland running on DP-1 it would conflict; it is not a parallel option while Hyprland owns the card. Confidence: medium.

## 5. Workarounds if leasing stays blocked

1. **Windowed/fullscreen Monado on the headset output** (Monado's "extended mode"): the compositor renders to a window moved onto the headset output. Supported by Monado's compositor, but the headset is then not a real non-desktop output, so the picture goes to a desktop-mode monitor. Note the docs' extended-mode caveat. Not tested here.
2. **Force the flag in the kernel**: a quirk patch to add PVR 0x1019 to edid_quirk_list (issue #40), or a `drm.edid_firmware`/EDID override that sets the non-desktop bit. Requires a kernel rebuild or boot parameter. This is a system change, so it stays out of scope for this read-only task.
3. **Separate session on the headset**: run a second compositor on a different VT for VR (e.g. Sway, a wlroots compositor that exposes drm-lease) while Hyprland is stopped. Sway with lease support was reported to detect non-desktop outputs (personal test page, not authoritative).
4. **Nested session** (not useful for display direct mode, only for testing).

## 6. Recommended path (step by step)

1. Confirm the blocker and record it: DP-1 `non-desktop` = 0 (done). Keep monitors.lua unchanged for now.
2. Try the kernel-side fix first (cheapest, most stable): check whether upstream drm/misc has merged the PVR quirk (issue #40). If not, it is a candidate patch for a local kernel build, not for runtime.
3. Parallel test (needs user approval, not done here): stop Hyprland DP-1 use and run Monado with `XRT_COMPOSITOR_FORCE_WAYLAND_DIRECT=1 XRT_COMPOSITOR_PRINT_MODES=1` to see whether any connector is offered. Expected today: none for DP-1.
4. Headset firmware: the panel only lights after the host sends the video-prepare sequence and holds a stable 72 Hz signal for about 5.5 s. `pimax_video_prepare(h, fps)` (libpimax/src/pimax.c:194) sends `5b 05 02 02`, `29 00 01`, `1a 05 02 FPS`, then `1a 05 05 00`. It is never called by the CLI (libpimax/README.md:26). **Unverified:** the 5.5 s figure is not in the repo; confirm from the capture logs (analysis/ or captures/) before relying on it.
5. Order matters: run the prepare sequence, then start the 72 Hz stream (7104x3840@72, DSC) before the lease or mode set, otherwise the leased output may come up black.
6. Only after non-desktop is visible in `modetest` (value 1) and Hyprland offers it, switch Monado to `XRT_COMPOSITOR_FORCE_WAYLAND_DIRECT=1` with `XRT_COMPOSITOR_DESIRED_MODE` set to the 7104x3840@72 index (currently index 2 in the modetest listing).
7. Keep DP-1 disabled in Hyprland (`disabled = true`) during the lease. Note that a disabled rule blocks the offer (Monitor.cpp), so the rule must be dropped to a non-disabled entry only if the kernel flag is fixed. Re-check this at that point.

## Confidence summary

- DP-1 non-desktop = 0 (high), PVR quirk absent upstream (high), Hyprland offers only nonDesktop outputs (high, code read), disabled rule blocks offer (high, code read), Monado Wayland direct needs VK_EXT_acquire_drm_display (high, code), NVIDIA 610 supports it (low, not verified), 5.5 s prepare delay (low, not in repo), Sway/Monado extended-mode workarounds (low).

## Sources

- https://raw.githubusercontent.com/torvalds/linux/master/drivers/gpu/drm/drm_edid.c
- https://gitlab.freedesktop.org/drm/misc/kernel/-/issues/40
- https://github.com/hyprwm/Hyprland (src/output/Monitor.cpp 285-318; src/protocols/DRMLease.cpp; src/managers/ProtocolManager.cpp:243)
- https://github.com/hyprwm/aquamarine (src/backend/drm/DRM.cpp:2077-2080; src/backend/drm/Props.cpp:33)
- https://gitlab.freedesktop.org/monado/monado (src/xrt/compositor/main/comp_window_direct_wayland.c; comp_settings.c; comp_window_direct.c)
- https://monado.freedesktop.org/direct-mode.html
- https://forums.developer.nvidia.com/t/nvidia-proprietary-non-open-modules-completely-unable-to-acquire-a-drm-lease-on-any-display-server-all-known-nvidia-drivers-any-hardware/341244
- https://gaiasky.space/news/2026/gaiasky-vr-linux-monado
- Local: /home/user/pimax-re/libpimax/src/pimax.c:194-206; libpimax/README.md:26
