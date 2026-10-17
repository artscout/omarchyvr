# Uninstall omarchyvr

Unofficial. Supports the Pimax Dream Air. Not affiliated with Pimax or Omarchy.

## Look first: `--dry-run`

    ./uninstall.sh --dry-run

It lists everything that would be removed, restored or kept. It changes nothing and asks for no password.

## What the uninstaller does to the system

The root part is one job with one password prompt. It first takes a snapper snapshot of `/` (when snapper has a `root` config). Then it:

- removes the udev rule `/etc/udev/rules.d/70-pimax.rules`;
- removes the two pacman hooks and `/etc/omarchyvr/nvidia.conf`, and restores the NVIDIA driver source in `/usr/src` to stock (from the `*.pimax-orig` backups, or by reversing the patches), then rebuilds the stock module with DKMS;
- removes the EDID override files (`/etc/pimax/`, `/etc/modprobe.d/nvidia-pimax.conf`, `/etc/mkinitcpio.conf.d/pimax.conf`) and rebuilds the initramfs;
- removes `cap_sys_nice` from the installed `monado-service` and SteamVR's `vrcompositor-launcher`;
- puts back system files that omarchyvr replaced and that were not its own (saved under `/var/lib/omarchyvr/backup`), then removes `/usr/local/lib/omarchyvr` and `/var/lib/omarchyvr`.

It says when a reboot is needed (changed NVIDIA module or boot image). If patch code is still found in the NVIDIA source after the restore, it says so loudly instead of hiding it.

## Kept and removed

Kept by default: your device data (factory calibration, controller models, lens, IPD, eye calibration, gyro bias, room map), `ai.toml` (cloud AI keys), downloads (AI models, voice models, backgrounds, hand models), your recordings, Steam and SteamVR, and packages the installer added with pacman. Settings are saved into `~/omarchyvr-settings-backup-<date>.tar.gz` and then reset. Options below remove more.

## No display after a driver update?

If the desktop or the headset picture is wrong after a driver update, try these in order.

1. The pacman hooks build the stock module when the patched build fails, so the desktop should still come up. Open [NVIDIA-UNTESTED.md](NVIDIA-UNTESTED.md) for what the hooks do on a new driver version.
2. If you have no picture at all, switch to a text console with Ctrl+Alt+F3 and log in.
3. In the clone, run `./uninstall.sh --yes`. It restores the stock NVIDIA source and module, removes the EDID override and rebuilds the initramfs. Reboot afterwards. (Not verified: the password prompt on a bare text console. The root job needs a polkit agent for `pkexec`.)
4. Or boot the snapper snapshot from the boot menu. The install and the uninstall each take one before they change anything, if snapper has a `root` config.

## Steps in detail

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
