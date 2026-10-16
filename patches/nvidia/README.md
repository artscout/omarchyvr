# NVIDIA display patches

Three patches for the NVIDIA open kernel modules (`open-gpu-kernel-modules`, tested on 610.57.04) that the Dream
Air's merged 2-head display needs, plus the pacman hooks that re-apply them after a driver update.

| File | What it does |
|---|---|
| `nvidia-2h1or-viewport.patch` | viewport x offset of merged heads (else the left half shows in both eyes); NVIDIA PR #1262 |
| `nvidia-2h1or-flip-fix.patch` | merged-head flip sync (module parameter `merged_head_sync`) |
| `nvidia-evo-makeroom-bounded-wait.patch` | bounded wait in the EVO make-room path (no display-engine hang) |
| `nvidia-patch-reapply.sh` | applies them idempotently to `/usr/src/nvidia-<version>` (`--check`, `--verify`, `--rebuild`) |
| `nvidia-pimax-patch.hook`, `nvidia-pimax-verify.hook` | pacman hooks (before and after the dkms build) |
| `test_nvidia_patch_reapply.sh` | test of the re-apply script on scratch copies, no root |

Apply by hand: `patch -p1 -d /usr/src/nvidia-<version> < nvidia-2h1or-viewport.patch` (in the order above), or let
the app's Fix button do it. Details: [../../linux/DISPLAY.md](../../linux/DISPLAY.md). The files are also reachable
as symbolic links in `linux/` (the app and the doctor look there).

Licence: the NVIDIA open kernel modules are MIT / GPL-2.0 (dual); these patches are offered under the same dual
licence.
