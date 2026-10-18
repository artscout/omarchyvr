# NVIDIA 615.78.08: ported patches (compile-checked, NOT tested on hardware)

Arch ships `nvidia-open-dkms` 615.78.08 (checked 2026-10-18). The three display patches in `patches/nvidia/` were written
and tested on 610.57.04 only. On 615.78.08 the viewport and make-room patches apply with `--fuzz=0` as they are, but the
flip-fix patch fails 4 hunks (context moved), and fuzz 2 would hide that. So the patches were ported properly.

State: **ported, applies with `--fuzz=0`, the patched open modules compile, never run on the headset.** The version is
listed in `patches/nvidia/ported-versions.txt` and **not** in `tested-versions.txt`: it stays an untested driver (the
app's switch "Allow the patch on untested NVIDIA drivers" is needed), but the opt-in applies the clean port and never the
relaxed `--fuzz=2` retry. The status file gets an extra line `port=ported`.

## Where the files are

| File | Meaning |
|---|---|
| `patches/nvidia/615.78.08/nvidia-2h1or-viewport.patch` | port of the viewport patch (same change, offset -69 lines) |
| `patches/nvidia/615.78.08/nvidia-2h1or-flip-fix.patch` | port of the flip-fix patch (3 context fixes, see below) |
| `patches/nvidia/615.78.08/nvidia-evo-makeroom-bounded-wait.patch` | port of the make-room patch (offsets only) |
| `patches/nvidia/ported-versions.txt` | versions that have such a directory |

Selection (`nvidia-patch-reapply.sh`, also used by the pacman hook and the root job): for driver version V each patch is
taken from `patches/nvidia/V/<name>` if that file exists, else from `patches/nvidia/<name>` (the default set, 610.57.04).
`--patch FILE` still overrides everything. The root job installs the per-version directories next to the default files
(`/usr/local/lib/omarchyvr/V/`), and the uninstall's reverse-apply uses the patches of the tree's own version.

## What differs between 610.57.04 and 615.78.08 around the patches

Compared function by function on the pristine sources. **No logic change in NVIDIA's code around merged heads, flips,
idling or make-room**; nothing in `nvidia-modeset` mentions merge/2Heads1OR differently (the only hit is a new argument
list of `nvDPValidateModeEvo`, unrelated). Details:

- **Viewport** (`nvkms-evo.c`, `nvSetViewPortsEvo`): function and `mergeHeadSection` use are byte-identical; only line
  numbers moved (-69).
- **Make-room** (`nvkms-dma.c`, `nvkms-dma.h`, `nvkms-types.h`): `nvEvoMakeRoom`, `nvDmaKickoffEvo` bodies identical.
  615 adds a RISC-V branch and an `#error "unknown arch"` to the write barrier in `nvDmaKickoffEvo` (not touched).
  `NVDmaBufferEvoRec` gets our `dead` field at a different offset in `nvkms-types.h` (+2 lines).
- **Flip-fix, kernel glue** (`nvidia-modeset-linux.c`, both `nvidia-modeset-os-interface.h`): the `merged_head_sync`
  parameter and the `nvkms_merged_head_sync()` declaration are now inserted after `debug` / `nvkms_debug_logging()`
  (615 added module parameters, `nvkms_max_output_color_bpc`, and extcon HDMI code in between). 615 also changed the
  permissions of NVIDIA's own `debug` (0600 -> 0644) and `fail_alloc_core_channel` (0400 -> 0444) parameters; ours stays
  `0644`, runtime-writable, default 1.
- **Flip-fix, `nvkms-hw-flip.c`**: `LimitMergedHeadSyncObjects` and its call in `nvUpdateFlipEvoHwState` have
  unchanged surroundings (offset -34). The diagnostics block (`TraceMergedHeadFlip`, `nvLogStuckLayerChannel`) moved from
  "right after `UpdateHDR`" to "right before the `nvFlipEvoOneHead` comment", because 615 added `UpdatePassthrough()` and
  `UpdateHDMIVsifMetadata()` in between. The hook in `nvFlipEvoOneHead` is unchanged. Other 615 changes in the file
  (color passthrough, HDMI VSIF, the window-mode checks that were removed, single per-GPU VRR masks) do not touch the
  sync-object, merged-head or idle paths.
- **Flip-fix, idle paths** (`nvkms-rm.c` `SyncOneEvoChannel`, `nvRMIdleBaseChannel`, `nvkms-flip.c`
  `nvIdleLayerChannels`, HAL `ForceIdleSatelliteChannel[IgnoreLock]C3`): bodies identical; only offsets (+181).
- **Flip-fix, `nvidia-drm-drv.c`** (grant/revoke/postclose): the touched functions are identical; offset +30.
- `nvkms_merged_head_sync`, `NVKMS_MERGED_HEAD_SYNC_*` and the parameter name/semantics (`0` all heads, `1` primary
  only, `2` acquire on all and release on the primary) are unchanged.

## Compile check

Built with `make modules` (NVIDIA's README procedure, `nice -n 19`, `-j8`) against the Arch `linux-headers` 7.2.11 of the
same day, outside the repo and without loading anything: `nvidia.ko`, `nvidia-modeset.ko`, `nvidia-drm.ko`,
`nvidia-uvm.ko`, `nvidia-peermem.ko` all link; `modinfo nvidia-modeset.ko` shows
`parm: merged_head_sync: ... (uint)`; the strings of the diagnostics ("Stuck layer channel") and of the bounded make-room
("marking it dead") are in the module. With the host's GCC 16 the stock `nvidia-uvm` needs `KCFLAGS=-Wno-error=format`
(an NVIDIA `%d` format warning in `uvm_migrate_pageable.c`, unrelated to the patches).

Tests: `patches/nvidia/test_nvidia_patch_reapply.sh` and `installer/test_root_nvidia.sh` cover the per-version
selection with synthetic ported versions; with `NVIDIA_615_DIR=<stock 615.78.08 tree>` (and `NVIDIA_PRISTINE_DIR` for
610) they also apply, verify and reverse the real port at `--fuzz=0`.

## Hardware test plan (maintainer, before 615.78.08 moves to `tested-versions.txt`)

Only between sessions (`pgrep -x monado-service` empty), with a snapper snapshot taken first.

1. Upgrade to 615.78.08 (pacman). The hook must print no failure banner. In the app: setup > NVIDIA driver patch shows
   615.78.08, "NOT a tested version", with the switch on: "patch applied ... because you allowed it", fuzz 0, and
   `/var/lib/omarchyvr/nvidia-patch-status` has `result=untested-allowed`, `fuzz=0`, `port=ported`.
2. Reboot. `cat /sys/module/nvidia_modeset/parameters/merged_head_sync` is `1`;
   `modinfo nvidia-modeset | grep merged_head_sync` shows the parameter; `omarchyvr-doctor`/Ready checks green.
3. DP-1 is non-desktop and leasable; Monado/SteamVR Play reaches 7104x3840 at 90 Hz (also check 72 Hz).
4. Both eyes correct: the left eye shows the left half, the right eye the right half (the viewport patch); run the
   existing display test (`vk_display_test`) and a game scene with head motion.
5. No flip hang over 10 minutes of continuous Play (repeat once with `merged_head_sync` 0 and 1 to confirm the A/B
   still behaves as on 610; 0 is expected to hang eventually, 1 must not), and over Play -> stop -> Play cycles.
6. `dmesg -T | grep -iE 'Xid|nvidia-modeset|Idling display engine|Stuck layer channel|marking it dead'` is empty
   during the run (a "Stuck layer channel" or "marking it dead" line is a failure of the flip fix or a real display stall).
7. Desktop sanity after Stop: Hyprland keeps working, Sunshine keeps streaming, no black output on the main screen.
8. Uninstall path: `omarchyvr-root.sh restore-stock` on the 615 tree restores the stock source (no `merged_head_sync`
   left), and a stock dkms rebuild succeeds.

If all pass, move `615.78.08` from `ported-versions.txt` to `tested-versions.txt` (keep the directory: it is still the
patch set for that version). If a step fails, keep the version untested and file the dmesg lines with the report.

Risks the test must cover: the flip semantics were reasoned from identical code, not observed; NVIDIA's other 615
changes (color passthrough, VSIF, flattened VRR masks) change the surrounding flip code; kernel 7.2.x vs the headers used
for the compile check; Xid or DSC link-rate behaviour of the 615 driver/GSP firmware (not examined).
