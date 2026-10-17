# Install omarchyvr

Unofficial. Supports the Pimax Dream Air. Not affiliated with Pimax or Omarchy.

## From clone to SteamVR

    git clone --recursive https://github.com/laughtt/omarchyvr.git
    cd omarchyvr && ./install.sh

That is the only command. Claude Code can run it for you. Everything after it happens in the app, by itself.
The terminal ends with "The app finishes the setup (one password)": it never asks for a password.

| # | Where | What happens | You press | Password |
|---|---|---|---|---|
| 1 | terminal (or Claude) | `./install.sh`: it checks this PC and builds everything (about 15-30 min the first time on a fast PC, at the lowest priority, so you can keep working). It installs the set and opens the app. | nothing after the command | no |
| 2 | app, "Setting up your PC" | first **"Which headset do you have?"** (USB detection only pre-selects, e.g. "Found on USB: Pimax Dream Air (asleep)"; you press **Continue**), then **"Get the headset ready"** (cables, power; live ticks where the PC can tell; **Start setup** works even if a tick is missing). Then it runs by itself: looks at this PC, fixes what needs no password, runs the system part (live list of steps), checks again | Continue, Start setup | **one** (pkexec window of your desktop) |
| 3 | app | all green and the job said "reboot" (NVIDIA module, boot image): **Restart now**. The app opens again at login and finishes the checks by itself (patched module loaded, EDID override, headset display mode, realtime permission) | **Restart now** (only when a restart is needed) | no |
| 4 | app, Ready | **Play**: SteamVR starts with the headset | **Play** | no |

All green and no restart needed: the only press is Play. If an item fails, it shows why and has one **Fix** button
(several root fixes: only **Fix all (one password)**, so one password covers them); a closed password window or a
wrong password shows **Try again**. The setup is not Ready while a part of the headset is missing: its control chip
(`34a4:0044`, the headset sleeps), its cameras (`34a4:0014`: the setup restarts the headset once) or its eye tracker
(`2104:0561`: switched on with the captured enable, once). "Get the headset ready" shows under the list and the checks
run again by itself when the parts are back. The display-mode Fix waits (no password) while the headset sleeps. When
the system part already asked for a restart, **Restart now** stays on that screen too.
IPD, controllers, eye calibration and the room map are optional ("Next, in the headset" on the Ready screen).
**Run setup again** is on the Ready screen. How it works: [GUI.md](GUI.md#setting-up-your-pc-the-automatic-setup-was-onboarding).

The headset's factory calibration (`~/.config/omarchyvr/headset-calibration.xml`, private, mode 600) comes with every
reinstall: the uninstaller keeps it, and `install.sh` copies it once from an older place (`~/.config/pimax/`, the lab
`analysis/` folder) when the new place has none. If none exists, Work and Play say "Headset calibration missing: press
Room setup", and the **Room setup** button (third tile, under Play and Work) reads it from the headset, then maps your
room (docs/ROOM-SETUP.md).

Later Plays never ask for a password. The realtime permission (`cap_sys_nice`) sits on the installed
`monado-service` at a fixed path (`~/.local/lib/omarchyvr/monado/...`). Builds in other directories never relink
that file, so the permission stays. An update (`./install.sh` again) keeps it too when Monado's binary did not
change: the installer keeps the old file when the rebuilt one has the same bytes (a relink alone used to drop the
permission, 2026-10-17). Only an update that really changes Monado loses it. Then the app checks this PC once at its
next start (install.sh leaves `~/.local/share/omarchyvr/post-install-check` for it) and shows the one-password Fix
right away, before anyone presses Play. All green: nothing is shown. That check looks at the PC only; a headset that
sleeps while you update is fine.

**Do it in one sitting.** Between an uninstall and the setup's password step the headset has no keep-alive (the
access rule comes with that step). Left like that for hours, its control chip restarts every few seconds and then
switches itself off (2026-10-17: four hours, then only a power cycle helped). Run `./install.sh` and let the app
finish the setup right after the uninstall; do not do other work in between.

On a PC that does not have the build packages yet, `./install.sh --install-packages` asks once more, before the
build, to install them with pacman. A build cannot start without them. Without the option, the installer lists
them and stops.

The local AI models (llama.cpp runtime, Lara, Qwen3.5-4B: about 5.3 GB) are no longer downloaded by the installer:
the first **Work** asks which voice assistant and speech engine you want and downloads only that, with progress and
Cancel (docs/WORK-FIRST-RUN.md). Play never needs them. Models already on disk are kept and found.

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

### The one root job (the app runs it by itself with one password; Show details has this list as text)

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
  in the app"); the rest of the setup is not affected. **Fix** runs the same step alone
  (`omarchyvr-root.sh edid <plan>`, one password, exit 3 when the display is still not seen). The app also switches an
  empty `OVR_EDID` to `auto` before the job when the headset is on USB.
- **The headset is chosen in the app, not guessed** (2026-10-17). A sleeping Dream Air (no keepalive yet) hides
  `34a4:0044` and shows only `34a4:0043` + `34a4:0014` and/or the eye chip `2104:0561`; the installer treats that
  signature as the tested Dream Air ("asleep: it wakes after the system setup", `installer/lib.sh headset_state`,
  test `installer/test_headset_detect.sh`), and when it cannot tell it no longer decides "NVIDIA not needed": the plan
  says `OVR_HEADSET=app`. The app's first screen saves the choice (`headset = dream-air | other` in `gui-state.toml`)
  and rewrites `root-plan.env`/`.txt` from it right before the password step: Dream Air = NVIDIA patch on a tested or
  allowed driver + `OVR_EDID=auto` (waits up to 60 s for a sleeping or unseen headset, 30 s for an awake one); "Other /
  not listed" = no NVIDIA and no EDID step, the app is still installed.
- AMD and Intel GPUs: no patches, marked experimental.

## Troubleshooting: the headset does not wake up

Symptom: during "Get the headset ready" the app says the headset is asleep or off, and `34a4:0044` (the headset's HID chip) is not on USB; only `34a4:0043` (bridge), `34a4:0014` (camera) and `2104:0561` (eye chip) are. Without a host the chip restarts every few seconds and after about two hours it switches itself off; a PC restart does not bring it back.

The app guides you and notices success by itself (green tick):
1. Step A: press the power button on the headset (a short animation shows where). It waits up to 90 s.
2. Step B, the last resort, only when no VR session runs: unplug the headset's POWER cable, wait 10 seconds, plug it back in. Leave the DisplayPort cable in. The animation counts the 10 s.
3. If it still does not appear, the app warns and Start setup stays possible: the setup installs the permission rule and the keep-alive, which can wake it. If the display is not found afterwards, the result screen offers the one Fix for the display mode (it asks again every 5 s by itself).

The cameras are missing (`34a4:0014` not on USB while the headset is on, the headset's USB3 side is off)? A Work or
Play start with head tracking restarts the headset ONCE (its own restart command; the keep-alive is back the moment
the headset leaves USB) and waits up to 60 s. If they stay off, the start stops and the app opens "Get the headset
ready": the cameras need a power cycle of the headset (the power button first, the power cable as the last step). A PC
restart does not bring them back. Real start 2026-10-17 13:07: the old recovery waited 150 s with the keep-alive
stopped, the headset restarted itself every 6 s, a second reset followed, and only the power cable helped.

The eye tracker (`2104:0561`) is not on USB after a headset restart: that is normal. The eye module is off until the
captured enable, which the VR start sends itself; the start says "Eye tracker off: switched on at the start". Do not
turn eye tracking off for it.

The app window did not open at login? A second start of the app brings the window to the front; if the window process has no window or does not answer within 3 seconds, it is replaced by a new one. At login the app waits for the Wayland display first. Log: `~/.local/state/omarchyvr/logs/omarchyvr-app.log`.

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
`--ai-models` (download the local AI models during the install, for agents and tests; by default the first Work
asks), `--no-ai` (the default now, kept for old scripts), `--lab-data DIR`.

## Sources of the downloads

All pinned in `installer/components.lock`. Flagged (`SOURCE=assumed`): the speech models (Parakeet v3 int8, Silero
VAD) and ONNX Runtime. The PC they were tested on got them by hand, and the URLs are the upstream releases. The
checksums of the tested files decide: a mismatch is refused. The Mercury hand models are never redistributed. They
are fetched from upstream only with `--hand-models`.
