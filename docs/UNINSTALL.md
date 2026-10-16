# Uninstall omarchyvr

In the app: **Ready > Uninstall omarchyvr** (it runs the same script). Without the app, run `./uninstall.sh` in the clone.

| Step | What happens | Password |
|---|---|---|
| 1 | stops the app and the user services (never during a VR session: stop VR first) | no |
| 2 | undoes the OpenXR/OpenVR/Steam registration (`steam-setup off`, Monado's driver entry removed from `openvrpaths.vrpath`; SteamVR's own runtime link stays) | no |
| 3 | reverses the install manifest, newest entry first: every file it lists is removed, every original it replaced is put back | no |
| 4 | removes what installs from before the manifest left behind (`installer/legacy-inventory.tsv`: the old `pimax-*` names, units, links) | no |
| 5 | settings and calibration: saved into `~/omarchyvr-settings-backup-<date>.tar.gz` (mode 600), then removed. `ai.toml` (your cloud AI keys) is kept | no |
| 6 | the root job: udev rule, pacman hooks and patch set, stock NVIDIA source (`*.pimax-orig` restored, dkms rebuilt stock, a hand-made `/usr/src/nvidia-<v>.orig` copy removed only if it equals the stock source), EDID override and initramfs entries, file capabilities, `/usr/local/lib/omarchyvr`, `/var/lib/omarchyvr` | **one** |
| 7 | says "reboot needed" when the NVIDIA module or the boot image changed | no |

Kept by default: downloads (`~/.local/share/omarchyvr/{ai,backgrounds}`, `~/models`, the Mercury hand models) and
your recordings (`slam-recordings`, `tracking-recordings`, `~/Videos/pimax`). Downloads left in the old
`~/.local/share/pimax` are moved to `~/.local/share/omarchyvr`, so the next install reuses them. Steam and SteamVR
are never removed. Packages that the installer added with pacman stay.

Options: `--dry-run` (list everything, change nothing, no password), `--remove-downloads`, `--keep-settings`,
`--remove-keys`, `--remove-lab-builds` (the lab's source trees and build dirs in `~/src`, about 150 GB; by default
they are only listed), `--no-root`.
