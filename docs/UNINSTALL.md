# Uninstall omarchyvr

In the app: **Ready > Uninstall omarchyvr** (it runs the same script). Without the app, run `./uninstall.sh` in the clone.

| Step | What happens | Password |
|---|---|---|
| 1 | stops the app and the user services (never during a VR session: stop VR first) | no |
| 2 | undoes the OpenXR/OpenVR/Steam registration (`steam-setup off`, Monado's driver entry removed from `openvrpaths.vrpath`; SteamVR's own runtime link stays) | no |
| 3 | reverses the install manifest, newest entry first: every file it lists is removed, every original it replaced is put back | no |
| 4 | removes what installs from before the manifest left behind (`installer/legacy-inventory.tsv`: the old `pimax-*` names, units, links) | no |
| 5 | settings: saved into `~/omarchyvr-settings-backup-<date>.tar.gz` (mode 600, never with `ai.toml` in it), then removed. Kept in place: `ai.toml` (your cloud AI keys) and your device data, which is not a setting: `headset-calibration.xml` (factory calibration, mode 600), the controller models (`controllers/`), `lens.conf` and `lens-ipd/`, `ipd.env`, `eye-calibration.toml`, `eye-correction.toml`, `ctrl-cam-extrinsics.json`, `ctrl-imu.json`, `slam-imu.json`, the gyro bias (`~/.cache/omarchyvr/gyro-bias`) and the room map (`~/.local/share/omarchyvr/roommap/`); an older copy under `~/.config/pimax/` comes back in the new place. `--reset-calibration` removes the device data too (it is in the tar) | no |
| 6 | the root job: udev rule, pacman hooks and patch set, stock NVIDIA source (`*.pimax-orig` restored, then any patch still in a file without a backup is reversed with `patch -R --fuzz=0` and `/usr/src/nvidia-<v>.orig` is used as the reference, then no patch marker may remain, else the job says so loudly; dkms rebuilt stock; `/etc/omarchyvr/nvidia.conf` and the patch status file removed, a hand-made `/usr/src/nvidia-<v>.orig` copy removed only if it equals the stock source), EDID override and initramfs entries, file capabilities, `/usr/local/lib/omarchyvr`, `/var/lib/omarchyvr` | **one** |
| 7 | says "reboot needed" when the NVIDIA module or the boot image changed | no |

Kept by default: the headset calibration `~/.config/omarchyvr/headset-calibration.xml` (the next install and the Room setup button need it; if it is ever lost, Room setup reads it from the headset again), downloads (`~/.local/share/omarchyvr/{ai,backgrounds}`, `~/models`, the Mercury hand models) and
your recordings (`slam-recordings`, `tracking-recordings`, `~/Videos/pimax`). Downloads left in the old
`~/.local/share/pimax` are moved to `~/.local/share/omarchyvr`, so the next install reuses them. Steam and SteamVR
are never removed. Packages that the installer added with pacman stay.

Options: `--dry-run` (list everything, change nothing, no password), `--remove-downloads`, `--keep-settings`,
`--reset-calibration`, `--remove-keys`, `--remove-lab-builds` (the lab's source trees and build dirs in `~/src`, about
150 GB; by default they are only listed), `--no-root`.

The next `./install.sh` brings back what an older install left elsewhere on this PC (`omarchyvr import-previous`, never
overwriting): docs/INSTALL.md "What a clean install keeps and imports". Test: `installer/test_uninstall_device_data.sh`.
