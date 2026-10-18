# NVIDIA patch for the Pimax Crystal Super on RTX 50 (Blackwell)

| File | What it does |
|---|---|
| `nvidia-dp-fec-reread.patch` | DP library: re-read the sink's FEC capability in the SST DSC check when the cached value says no, and do not dereference a missing native device there |

Tested on: 595.99.02 open modules, RTX 5090, Ubuntu 26.04 (kernel 7.0), Pimax Crystal Super Micro-OLED, 2026-10-18.
Also applies to 615.71.09 (`patch -p1`, offset 137 lines; not tested live).

## Why

The Crystal Super's only modes are 7104x3840 at 90 Hz (2564 MHz) and 72 Hz (2028 MHz). Its link is DP 1.4 HBR3 x4
(no UHBR), so both need DSC (sink: DSC 1.2, max 8.0 bpp, FEC). Without the patch nvidia-drm lists no mode for the
headset ("Mode is rejected: DP Bandwidth check failed"), because:

1. The DP library reads the sink's FEC capability (DPCD 0x90) once, when it detects the sink. The headset's display
   bridge still NAKs AUX reads at that moment ("AuxChCtl Failing"), the cached answer stays "no FEC", and
   `compoundQueryAttachSSTIsDscPossible` refuses DSC for the whole session. The patch reads FEC again there.
2. The same function dereferenced the native device without a check. A mode can be validated before the DP library
   has found the sink (with an EDID override, during the boot-time fbdev probe): that was a NULL dereference. The
   patch adds the check.

A second fix is needed outside the driver: the headset's EDID says "colour depth undefined" (base block byte 20 =
0x85), NVKMS then validates at 6 bpc and DSC refuses 6 bpc. See `linux/crystal-super/` (EDID override, 8 bpc, plus a
Microsoft VSDB so the connector is non-desktop and the compositor leases it).

## The Dream Air patches on Blackwell

`nvidia-2h1or-viewport.patch` and `nvidia-2h1or-flip-fix.patch` change the 2Heads1OR (merged heads) paths. Blackwell
has no 2Heads1OR (`nvkms-evo4.c`: `supportsMergeMode = FALSE`); it drives wide DSC modes with one head in "multi-tile"
mode, so those two patches never run there and the Crystal Super does not need them.
`nvidia-evo-makeroom-bounded-wait.patch` is a general guard against a stuck display channel; it does no harm, but the
Crystal Super does not need it either. The Dream Air on an RTX 4090 (Ada) still needs all three. The Dream Air on
Blackwell has not been tried.

## Apply

The Ubuntu packages ship nvidia-modeset's core as a prebuilt object, so build the open modules from the matching tag
of https://github.com/NVIDIA/open-gpu-kernel-modules: `patch -p1 < nvidia-dp-fec-reread.patch`, then
`make modules SYSSRC=/lib/modules/$(uname -r)/build`, sign the five modules with an enrolled MOK key under Secure Boot,
and install them into `/lib/modules/$(uname -r)/updates/` (depmod prefers them over the stock ones). Steps:
`docs/CRYSTAL-SUPER.md`.
