# Install omarchyvr

Unofficial. Supports the Pimax Dream Air. Not affiliated with Pimax or Omarchy.

## From clone to SteamVR

    git clone --recursive https://github.com/laughtt/omarchyvr.git
    cd omarchyvr && ./install.sh

That is the only command. Claude Code can run it for you. Everything after it happens in the app.

| # | Where | What you do | Password |
|---|---|---|---|
| 1 | terminal (or Claude) | `./install.sh`: it checks this PC and builds everything (about 15-30 min the first time on a fast PC, at the lowest priority, so you can keep working). It installs the set and opens the app. | no |
| 2 | app, setup wizard, "System setup" step | read the list of system changes, click **Set up this PC** | **one** (pkexec) |
| 3 | app | if the list said "reboot": reboot (only when the NVIDIA module or the boot image changed) | no |
| 4 | app, Home | **Play**: SteamVR starts with the headset | no |

Later Plays never ask for a password. The realtime permission (`cap_sys_nice`) sits on the installed
`monado-service` at a fixed path (`~/.local/lib/omarchyvr/monado/...`). Builds in other directories never relink
that file, so the permission stays. Only an update that rebuilds Monado asks again, inside that update's one
root job.

On a PC that does not have the build packages yet, `./install.sh --install-packages` asks once more, before the
build, to install them with pacman. A build cannot start without them. Without the option, the installer lists
them and stops.

## What gets installed

| Where | What | On uninstall |
|---|---|---|
| `~/.local/bin` | the whole set, installed together: `omarchyvr` (CLI), `omarchyvr-app` (the app), `omarchyvrd`, the tools (`omarchyvr-*`, `pimax-*` hardware tools, `xr-*` test clients) | removed |
| `~/.local/lib/omarchyvr/monado` | Monado from `forks/monado`, one build with every feature (Pimax driver, 6DoF, eye, controllers, anchor, recorder tap, camera hands) | removed |
| `~/.local/lib/omarchyvr/wayvr` | WayVR from `forks/wayvr` | removed |
| `~/.local/lib/omarchyvr/{basalt,onnxruntime,sherpa-onnx}` | Basalt (6DoF), ONNX Runtime (hands), the speech library; all pinned in `installer/components.lock` | removed |
| `~/.local/share/omarchyvr/data` | docs, udev rule, EDID files, NVIDIA patches, the installer (the app reads these) | removed |
| `~/.local/share/omarchyvr/manifest.tsv` | the install manifest: every file with its checksum, and the originals it replaced | removed last |
| `~/.local/share/omarchyvr/{ai,backgrounds}`, `~/models` | downloads (AI runtime and models, VR backgrounds, speech models) | **kept** (unless `--remove-downloads`) |
| `~/.config/systemd/user/omarchyvr-*.service`, `omarchyvrd.service` | user units: keepalive and omarchyvrd enabled; autod (replaced by omarchyvrd) and the AI server (started by the app's AI switch) installed only | removed |
| `~/.config/autostart/omarchyvr-app.desktop`, `~/.local/share/applications/omarchyvr.desktop`, icon | app at login, launcher | removed |
| OpenXR/OpenVR registration | SteamVR is the OpenXR runtime; Monado's SteamVR driver is registered in `openvrpaths.vrpath`; the Steam entry gets the OpenXR import variable | undone |

### The one root job (the wizard shows it before you type the password)

| Change | When | Undo |
|---|---|---|
| snapper snapshot of `/` | when snapper has a `root` config | (it stays: your restore point) |
| `pacman -S` of missing runtime packages | only if missing | packages stay (other software may use them) |
| `/etc/udev/rules.d/70-pimax.rules` + `/usr/local/lib/omarchyvr/pimax-hub-power.sh` | always | removed |
| NVIDIA patches + pacman hooks `65-nvidia-pimax-patch.hook`, `75-nvidia-pimax-verify.hook` + `/usr/local/lib/omarchyvr` | NVIDIA `nvidia-open-dkms` on a **tested** version, a Dream Air plugged in (or `--nvidia yes`) | stock source restored from the `*.pimax-orig` files, dkms rebuilt stock |
| EDID override `/etc/pimax/{dp-edid.bin,nvidia-modeset.conf}`, `/etc/modprobe.d/nvidia-pimax.conf`, `/etc/mkinitcpio.conf.d/pimax.conf`, initramfs rebuild | same, and the headset's connector was found | removed, initramfs rebuilt |
| `setcap cap_sys_nice+ep` on the installed `monado-service` | always | `setcap -r` |
| `setcap cap_sys_nice=eip` on SteamVR's `vrcompositor-launcher` (what SteamVR's own prompt does) | when SteamVR is installed | `setcap -r` |

Every file that is replaced and was not ours is saved under `/var/lib/omarchyvr/backup` and put back on uninstall.

### Why the realtime permission works this way

The options were (a) `setcap` once on a stable installed binary, or (b) a polkit rule that re-applies the cap with
no password. We chose (a). With (b), any process running as you could silently get `cap_sys_nice` for any program
placed at that path. With (a), a changed binary loses the cap (the kernel clears it on write), and a person typed
the password for the exact file. Play never relinks the installed file, so it never asks again.

## NVIDIA policy (strict)

- Patched by default only on the exact tested versions in `patches/nvidia/tested-versions.txt` (`610.57.04`), open kernel
  modules only. Another version is patched only if you turn on **Allow the patch on untested NVIDIA drivers** in the app
  (off by default, `--nvidia-allow-untested` for `./install.sh`); see [NVIDIA-UNTESTED.md](NVIDIA-UNTESTED.md).
- Before patching, `nvidia-patch-reapply.sh --check` tests each fix against the source. A fix that is already in
  (for example NVIDIA PR #1262, the viewport fix, once it ships) is skipped.
- Untested version, switch off: no patch, the stock module is built, pacman is not blocked. The app says so in plain
  words and offers to send a report.
- Secure Boot on: patched only when the dkms signing key is enrolled (`mokutil --test-key`). Otherwise the module
  could not load.
- If the patched dkms build fails, the stock source is restored and the stock module is built, so the desktop keeps
  working. This holds for the installer and for the pacman hook on driver updates.
- The EDID override key (`override.[<PCI address>].<NVKMS connector>`) is detected on your PC from the headset's
  EDID. It is never copied from ours.
  A fresh PC does not show the headset's display yet (it sleeps until the keepalive wakes it, and that needs the
  headset access rule from the same job). So when the headset is on USB but no display is visible, the installer
  writes `OVR_EDID=auto` and the system setup looks for the connector itself: it installs the access rule first, then
  waits up to 30 s for the headset's EDID, then writes the override. One password, one reboot. If the display never
  shows up, only the EDID step is skipped ("headset display not seen: put the headset on / turn it on, then press Fix
  in the app"); the rest of the setup is not affected.
- AMD and Intel GPUs: no patches, marked experimental.

## Requirements

| Item | Tested | Experimental | Unsupported |
|---|---|---|---|
| Distro | Omarchy (Arch) | other Arch-based | non-Arch (no pacman hooks or packages) |
| Compositor | Hyprland (Lua config) | other Wayland compositors with `wp_drm_lease_device_v1` | X11 |
| GPU | NVIDIA RTX 4090 | other NVIDIA RTX with the open modules; AMD, Intel (no patches) | NVIDIA proprietary-only modules (pre-Turing) |
| NVIDIA driver | `nvidia-open-dkms` 610.57.04 | (none) | any other version: runs unpatched (the left half shows in both eyes) unless you allow the patch on untested drivers (app switch, off by default) |
| Headset | Pimax Dream Air (34a4:0044) | other Pimax models: read-only probe, labelled "untested" | |
| Kernel | 7.2 (linux-omarchy) | other Arch kernels with headers | |
| Disk | 40 GB free for the build, plus ~30 GB of downloads (reused when present) | | |
| Steam | Steam (multilib) + SteamVR, installed by you | | |

## Options

`./install.sh --help` lists all of them. Main ones: `--prefix DIR` (staging install, no system changes),
`--no-root`, `--dry-run-root`, `--nvidia auto|yes|no`, `--nvidia-allow-untested`, `--gpu-speech` (whisper.cpp on Vulkan for the
"Speech on graphics card" switch), `--hand-models` (Mercury models from upstream, no licence, personal use only),
`--no-ai`, `--lab-data DIR`.

## Sources of the downloads

All pinned in `installer/components.lock`. Flagged (`SOURCE=assumed`): the speech models (Parakeet v3 int8, Silero
VAD) and ONNX Runtime. The PC they were tested on got them by hand, and the URLs are the upstream releases. The
checksums of the tested files decide: a mismatch is refused. The Mercury hand models are never redistributed. They
are fetched from upstream only with `--hand-models`.
