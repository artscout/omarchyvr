# NVIDIA Linux: why a Pimax Dream Air exposes zero modes (research)

Scope: web research only. Nothing on the machine was changed. Dates are as found in sources.
Confidence: H = primary source read, M = secondary or search snippet, L = inferred or not found.

## Key facts from the kit EDID (for context)
- Product 4121 = 0x1019. Vendor PVR (0xD242).
- 0x1019 is exactly the product ID in an unmerged kernel quirk proposal for PVR (see Q5).
- Range descriptor max dotclock = 1000 MHz. The DisplayID-only modes are 2170 MHz and 2642 MHz, so they sit
  far outside the EDID range limits. This is the most likely reason a validator rejects them (see Q1).

## Q1. Does nvkms reject modes outside the range descriptor, and/or fail to parse DisplayID-only timings?
- Partly confirmed. NVIDIA's X ModeValidation option has per-stage tokens including `NoEdidMaxPClkCheck` and
  `NoMaxPClkCheck` (M, Arch BBS / NVIDIA forum snippets, not fetched). These are the stages that would reject
  a 2.1-2.6 GHz mode against a 1000 MHz EDID limit. Whether nvkms on KMS runs the same checks is unverified (L).
- Digital displays allow only EDID-advertised modes by default. `AllowNonEDIDModes` lifts that (M, NVIDIA
  forum/Arch snippets).
- No NVIDIA statement on DisplayID-only parsing was found (L).
- Known HMD failures on NVIDIA Linux, none with a confirmed fix:
  - Bigscreen Beyond: "cannot be used on either proprietary or open driver". Mode 3840x1920 seen via Vulkan WSI
    but not RandR. Error `VRInitError_Compositor_CannotDRMLeaseDisplay`. NVIDIA filed internal bug 4834531.
    Last reply Nov 6, 2024. Suspected DSC BPP handshake. Driver range tested 535-560 (H, NVIDIA forum 294108).
  - Valve Index and others: DRM lease fails on proprietary and open modules, 555-580.x, through Sept 2026. No NVIDIA
    staff reply. Open modules need Turing or newer (H, NVIDIA forum 341244).
- Fixed? Nothing found. The only working Bigscreen setup reported was AMDGPU with a DisplayID DSC patch (H).

## Q2. Override EDID or mode validation under NVIDIA + Wayland
- `nvidia-drm modeset`: confirmed. Default enabled. Disable with
  `modprobe -r nvidia_drm; modprobe nvidia_drm modeset=0` (H, NVIDIA README 595.84 kms.html).
- `nvidia_modeset conceal_vrr_caps`: confirmed param exists (H, same page). Not relevant to modes.
- `NVreg_RegistryDwords`: confirmed as a generic NVIDIA module option. Format is `NVreg_RegistryDwords="key=val;..."`
  (M, NVIDIA forum / Nouveau commit notes). I found no registry key for EDID or mode validation.
- `nvidia-modeset config_file`: NOT FOUND. I could find no evidence it exists. Treat as unverified (L).
- ModeValidation syntax: documented for X config (`Option "ModeValidation" "..."` in Device or Screen section),
  per NVIDIA README Appendix J (M). Not documented for KMS/nvkms.
- `drm.edid_firmware` with nvidia-drm: an Arch thread reports the kernel did NOT apply the firmware EDID at boot.
  Loading it via debugfs worked (M, search snippet). Treat as unreliable.
- Working path that the LVRA wiki describes for NVIDIA + Wayland: rewrite the EDID live with edid-json-tools
  (H, vronlinux.org Pimax page, P2-series text, same NVIDIA path):
  ```
  python3 -m pip install --editable .[CLI]    # in an edid-json-tools checkout
  sudo patch_edid override -d -m 1 DP-1      # run once per boot; adjust connector name
  ```
  The patch must be applied before Monado/Hyprland tries the display. Then restart the compositor/Monado.
- Env vars: none found that override EDID or modes on NVIDIA KMS (L).

## Q3. Userspace atomic modeset / DRM lease with a custom mode blob
- Not verified on nvidia-drm. Whether nvkms accepts a custom mode blob absent from the connector mode list is
  unknown (L). Expect nvkms to validate against EDID as in Q1 (L).
- Hyprland DRM lease: Hyprland implements wp_drm_lease_device_v1 (M; Gaia Sky guide, Phoronix). The DRM lease
  path is gated on the connector's non-desktop property. The non-desktop quirk must match for the headset to be
  leasable (H, Monado direct-mode doc).
- Hyprland user report: a July 5, 2026 thread on NVIDIA 610.43.02 says SteamVR error 496 and "DRM lease failure"
  with a Valve Index. No fix. The poster says linked issues #7776 and #8568 did not help (H, forum.hypr.land 2017).
- Hyprland custom modes: the wiki describes a `monitor = NAME, modeline ...` form in some versions (M). The
  0.54 wiki page was truncated and I did not confirm the syntax. Since 0.55, the config is Lua
  `hl.monitor({...})` (M, search snippet). Whether nvidia accepts a custom modeline is unverified (L).

## Q4. DSC on DP for 2.1-2.6 GHz pixel clocks, NVIDIA open driver
- The 4K-class HMD failure above is linked to DSC BPP (M). NVIDIA states nothing on this in the sources.
- NVIDIA filed bug 4834531 for the Bigscreen case. Staff said a similar Windows-driver DSC issue was fixed,
  "likely a regression in 550" (H, NVIDIA forum 294108).
- Driver versions tested by users: 535-560 (Bigscreen); 555-580.x and 610.43.02 (lease failures). No report
  specifically on 610.57.04 was found (L).
- Upstream DisplayID DSC passthrough patches (patchew 2025) say the NVIDIA open module "indicates this value"
  but it is unused on Linux. Secondhand (L).

## Q5. Monado on NVIDIA; Pimax; PVR quirk
- Monado NVIDIA direct mode (H, monado.freedesktop.org/direct-mode.html):
  - `Option "AllowHMD" "yes"` (xorg.conf) turns off the driver's HMD hiding. Only applies on X.
  - `XRT_COMPOSITOR_FORCE_NVIDIA_DISPLAY="<display name>"` adds a display to Monado's allowlist.
  - Wayland DRM lease: forced by `XRT_COMPOSITOR_FORCE_WAYLAND=1` if not auto-detected.
  - KMS backend: `XRT_COMPOSITOR_FORCE_VK_DISPLAY=N` selects the Nth connector. Needs a text VT.
  - Fallback: `XRT_COMPOSITOR_FORCE_XCB=1` (extended mode window).
- Pimax on Linux (H, vronlinux.org/docs/hardware/pimax/): only the P2 series is supported. Crystal is not. Patches:
  "non-desktop" and "ps0002 drm-edid fix checksum errors in Pimax HMD EDIDs". Use the pimax branch of Monado
  (AnnoyingRain5). Dream Air is not mentioned.
- Kernel quirk for PVR: proposed May 2024 by Sefa Eyeoglu. Adds `EDID_QUIRK_NON_DESKTOP` for PVR product IDs
  0x1019, 0x101a, 0x101b. Coreforge questioned it. NOT confirmed merged (M, freedesktop drm/misc/kernel issue 40;
  page blocked by bot protection, read via search snippet). Our product 4121 = 0x1019 matches.
  Check mainline `drivers/gpu/drm/drm_edid.c` for a `PVR` entry before relying on it.
- Test the quirk locally without a kernel build: `xrandr --prop` (Xorg) shows non-desktop. Wayland equivalent
  unverified.

## Actionable next steps (not performed)
1. Confirm the EDID checksum: `edid-decode /sys/class/drm/card1-DP-1/edid`. The Pimax checksum patch exists for a reason.
2. Check the upstream kernel `drm_edid.c` for a PVR 0x1019 non-desktop quirk. If absent, the EDID-override path
   is the workaround (Q2, edid-json-tools `patch_edid`).
3. Test with `patch_edid override` on DP-1, then check whether `/sys/class/drm/card1-DP-1/modes` lists 7104x3840.
   The 90 Hz mode may fail on the 1000 MHz dotclock limit while 72 Hz is closer. Check `NoMaxPClkCheck`-style behavior.
4. File an NVIDIA bug with the EDID and a `nvidia-bug-report.sh` dump. Reference bug 4834531 and forum 294108.

## Sources
- https://forums.developer.nvidia.com/t/bigscreen-beyond-hmd-unable-to-be-initialized-on-nvidia-possibly-due-to-dsc-bpp-issue/294108/1
- https://forums.developer.nvidia.com/t/nvidia-proprietary-non-open-modules-completely-unable-to-acquire-a-drm-lease-on-any-display-server-all-known-nvidia-drivers-any-hardware/341244
- https://download.nvidia.com/XFree86/Linux-x86_64/595.84/README/kms.html
- https://download.nvidia.com/XFree86/Linux-x86/1.0-8756/README/appendix-j.html (search snippet)
- https://forums.developer.nvidia.com/t/driver-refuses-to-accept-working-modeline-4k-120-dp-hdmi-2-1/165740 (search snippet)
- https://monado.freedesktop.org/direct-mode.html
- https://vronlinux.org/docs/hardware/pimax/
- https://gitlab.freedesktop.org/drm/misc/kernel/-/issues/40 (search snippet; page blocked)
- https://wiki.hypr.land/0.54.0/Configuring/Monitors/ (truncated)
- https://forum.hypr.land/t/steam-vr-error-496-on-launch-intel-nvidia-580xx-drivers/2017
- https://www.phoronix.com/news/Linux-Bigscreen-Beyond-VR
- https://www.phoronix.com/news/Wayland-DRM-Lease-Merged
