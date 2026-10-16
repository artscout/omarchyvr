# EDID that marks the Dream Air connector non-desktop (prepared 2026-10-13, NOT tested live)

Goal: kernel sets `connector->display_info.non_desktop` so Hyprland offers DP-1 for DRM lease to Monado.
Files: `linux/pimax-dp-edid-hmd.bin` (try first), `linux/pimax-dp-edid-hmd-dual.bin`, builder `linux/make_hmd_edid.py`.

## 1. Kernel (torvalds/linux master, drivers/gpu/drm/drm_edid.c, fetched 2026-10-13)
non_desktop is set in exactly three places (+ cleared at the start of each update, `info->non_desktop = 0;` ~L6700):
- DisplayID 2.0 primary use (`drm_displayid_process_base_section_header`, ~L6720-6734): version == 0x20 and
  primary_use == PRIMARY_USE_HEAD_MOUNTED_VR (7) or _AR (8) -> `info->non_desktop = true`.
  DisplayID 1.x never sets it (the Pimax stock EDID is 1.2, so no flag).
- Microsoft HMD VSDB in CTA (`drm_parse_microsoft_vsdb`, ~L6417-6427): version 1/2, or 3 without desktop-usage bit.
- Quirk list (`edid_quirk_list`, L230-274, `EDID_QUIRK_NON_DESKTOP`, applied at the `out:` label ~L6884):
  VLV Valve, HVR Vive, OVR Oculus, WMR (ACR/LEN/FUJ/DEL/SEC/AUS), SNY 0x0704, SEN 0x1019, SVR 0x1019, AUO 0x1111.
  **No PVR/0x1019** entry (EDID mfg is PVR: freedesktop drm/misc/kernel issue #40 unmerged).
- Only the FIRST DisplayID extension is used for this: `update_displayid_info` (~L6775) calls the header processor
  once (`base_section_header_processed`), and drm_displayid.c `__displayid_iter_next` records version/primary_use only
  `if (base_section)` (first section found). Later sections are iterated for blocks but not for primary use.
  => in the dual EDID the DisplayID 2.0 block must come BEFORE the 1.2 block (it does).
- Reaches userspace: `_drm_edid_connector_property_update` (~L7138) does
  `drm_object_property_set_value(..., non_desktop_property, display_info.non_desktop)`.

## 2. NVIDIA open modules 610.57.04
(a) nvidia-drm-connector.c: with an override EDID (`drm_edid_override_connector_update(connector) > 0`, L151) it passes it
    to NVKMS (`overrideEdid`); in `nv_drm_connector_get_modes` (L392-393) it calls
    `nv_drm_connector_update_edid_property(connector, nv_connector->edid)`, a wrapper (nvidia-drm-helper.h L344-351) for
    `drm_connector_update_edid_property()`, which in the kernel is `drm_edid_connector_update(drm_edid_legacy_init(...))`
    (drm_edid.c L7260-7265) = `update_display_info()` + property update. So YES: the kernel-parsed non_desktop reaches the
    property. nvidia-drm itself never mentions non_desktop (grep clean). CAVEAT: the EDID given to the kernel is
    `nv_connector->edid`, i.e. what NVKMS returned (the override EDID); modes still come from NVKMS (nvtiming), not the kernel.
(b) nvtiming: `parseDisplayId20EDIDExtSection` (nvt_edidext_displayid20.c) requires `section_bytes == 121`, version 0x20,
    section checksum 0; first DID2.0 section should have primary use 1..8 (check only enforced in unit tests).
    Type 7 (L336 tag only; parse in nvt_displayid20.c ~L595-665, descriptor ~L1560-1665): block data_bytes % 20 == 0,
    1..DISPLAYID_2_0_TIMING_7_MAX_DESCRIPTORS descriptors, rejects if HTotal < HVisible+HFrontPorch+HSyncWidth.
    Revision <ycc420 revision: bit 7 of options = preferred. pclk is stored in 1 kHz and 10 kHz units (24-bit, ok for 2.64 GHz).
    "Strong validation" wants product-id + display-params + interface-features + Type 7 blocks (flag only); we include the first
    three except interface-features. Still unverified: whether NVKMS applies the base-block range limit (55-65 Hz, 1000 MHz)
    to DID2.0 timings the same as DID1.2 (see analysis/dp-edid-linux.md); the base block is unchanged here.

## 3. Layouts (all 128-byte blocks; base+CTA identical to pimax-dp-edid-native.bin except base ext count/checksum)
- hmd.bin (384 B): [0] base (ext=2) [1] CTA [2] DisplayID 2.0: hdr 70 20 79 07 00 (use 7 = HMD VR, 121 payload bytes),
  blocks: 0x20 product id (PVR-ish code 0x1019, "Pimax"), 0x21 display params (7104x3840, 8 bpc), 0x22 Type VII x2.
- hmd-dual.bin (512 B): [0] base (ext=3) [1] CTA [2] DisplayID 2.0 (same) [3] DisplayID 1.2 (original, 2 Type I timings).
  Kernel checks block [2] first (2.0, HMD VR) -> non_desktop. NVKMS parses every DID ext, so it sees both timing sets
  (the same two modes twice; harmless dups). edid-decode warns "Expected 0 DisplayID ext, got 1" (ext count of block 2).
- Type VII: 72 Hz pclk 2170070 kHz, 7104x3840, H 400/48/200 +, V 29/1/18 +, preferred; 90 Hz 2642600 kHz, H 200/48/200 -, V 29/1/18 -.

## edid-decode 1.32.0 summary (both files)
`Version: 2.0`, `Display Product Primary Use Case: Head-mounted Virtual Reality (VR) display`, DTD 7104x3840 72.000203 Hz
(2170.07 MHz, preferred) and 90.000142 Hz (2642.6 MHz), all checksums good. `--check` still prints FAIL, but only for
inherited items that the current working native EDID also has: HDMI VSDB max TMDS 150 MHz vs timings, missing VIC 1/VCDB,
sRGB chromaticities, product name newline, plus "CTA native vs DisplayID native resolution" (3840x2160 base DTD vs
7104x3840 display-params). No DisplayID parse errors. Removing the HDMI VSDB would clear TMDS but changes is_hdmi/audio; not done.

## 4. Which to try first
`pimax-dp-edid-hmd.bin`: smallest change, single DisplayID. Fall back to `-dual` only if NVKMS shows no 7104x3840 modes
(i.e. NVIDIA wants 1.2 timings); the dual file keeps the kernel flag because the 2.0 block is first.

## LIVE TEST (user, sudo; nothing here was run). Never `echo off > status` (broke Hyprland CRTC state).
```
cd ~/omarchyvr/linux
sudo cp pimax-dp-edid-hmd.bin /sys/kernel/debug/dri/1/DP-1/edid_override   # or `cat f | sudo tee`
echo detect | sudo tee /sys/class/drm/card1-DP-1/status
modetest -M nvidia-drm -c | grep -A60 '^826'      # connector id 826 per notes; look for "non-desktop" value 1 and 7104x3840 modes
cat /sys/class/drm/card1-DP-1/modes | head; hyprctl monitors | grep -A3 DP-1   # DP-1 should vanish from Hyprland if leased-out
# restore
echo reset | sudo tee /sys/kernel/debug/dri/1/DP-1/edid_override
echo detect | sudo tee /sys/class/drm/card1-DP-1/status
```
If the debugfs path differs: `ls /sys/kernel/debug/dri/*/DP-1`. Try the dual file the same way.

## Permanent install (replaces the current native EDID in the boot path from DISPLAY.md)
```
sudo install -D -m644 linux/pimax-dp-edid-hmd.bin /etc/pimax/dp-edid.bin
sudo mkinitcpio -P && reboot      # nvidia-modeset.conf, modprobe and mkinitcpio FILES entries already exist
```
Undo: `sudo install -D -m644 linux/pimax-dp-edid-native.bin /etc/pimax/dp-edid.bin && sudo mkinitcpio -P && reboot`.
Note: as non-desktop, Hyprland will stop using DP-1 as a monitor (drop the DP-1 line in monitors.lua); desktop-on-headset needs the native EDID.
