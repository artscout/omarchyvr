# NVIDIA display patches

Three patches for the NVIDIA open kernel modules (`open-gpu-kernel-modules`, tested on the versions in
`tested-versions.txt`: today 610.57.04; any other version only on opt-in, see below) that the Dream
Air's merged 2-head display needs, plus the pacman hooks that re-apply them after a driver update.

| File | What it does |
|---|---|
| `nvidia-2h1or-viewport.patch` | viewport x offset of merged heads (else the left half shows in both eyes); NVIDIA PR #1262 |
| `nvidia-2h1or-flip-fix.patch` | merged-head flip sync (module parameter `merged_head_sync`) |
| `nvidia-evo-makeroom-bounded-wait.patch` | bounded wait in the EVO make-room path (no display-engine hang) |
| `tested-versions.txt` | the driver versions the patches were tested on (one per line) |
| `nvidia-patch-reapply.sh` | applies them idempotently to `/usr/src/nvidia-<version>` (`--check`, `--verify`, `--rebuild`, `--allow-untested`) |
| `nvidia-pimax-patch.hook`, `nvidia-pimax-verify.hook` | pacman hooks (before and after the dkms build) |
| `test_nvidia_patch_reapply.sh` | test of the re-apply script on scratch copies, no root |

## Untested drivers

Only the versions in `tested-versions.txt` are patched. On any other version `nvidia-patch-reapply.sh` prints a banner
("driver X is not tested ... turn on 'Allow the patch on untested NVIDIA drivers' in the app"), patches nothing and exits
0, so pacman is never blocked and dkms builds the stock module (the desktop works). With the opt-in (`--allow-untested`,
or `allow_untested=1` in `/etc/omarchyvr/nvidia.conf`, which the app's switch writes through the system setup job) it
prints a loud WARNING, tries `--fuzz=0` and then `--fuzz=2`, still requires the `merged_head_sync` marker, and with
`--fallback` a failed build ends with the stock module. Every run writes `/var/lib/omarchyvr/nvidia-patch-status`
(`driver=`, `result=tested|untested-allowed|untested-refused|failed`, `fuzz=`, `time=`) for the app. Tests can override
the paths with `NVIDIA_PATCH_TESTED`, `NVIDIA_PATCH_CONF`, `NVIDIA_PATCH_STATUS`. Details: [../../docs/NVIDIA-UNTESTED.md](../../docs/NVIDIA-UNTESTED.md).

Apply by hand: `patch -p1 -d /usr/src/nvidia-<version> < nvidia-2h1or-viewport.patch` (in the order above), or let
the app's Fix button do it. Details: [../../linux/DISPLAY.md](../../linux/DISPLAY.md). The files are also reachable
as symbolic links in `linux/` (the app and the doctor look there).

Licence: the NVIDIA open kernel modules are MIT / GPL-2.0 (dual); these patches are offered under the same dual
licence.
