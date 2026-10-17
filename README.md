# omarchyvr: Pimax Dream Air VR on Linux (Omarchy / Arch)

[Support the project on Ko-fi](https://ko-fi.com/josemauriciocarpioherrera) · [Follow on X](https://x.com/carpio_her8717)

**Unofficial community project.** It is not made, endorsed or supported by Pimax or Omarchy. "Pimax" and "Dream Air" are trademarks of Pimax, used only to say which hardware this works with. There is no warranty: the tools talk to the headset over USB and change system settings, so use them at your own risk. Code is MIT OR Apache-2.0, docs and data are CC-BY-4.0 (see [NOTICE](NOTICE) and [THIRD-PARTY.md](THIRD-PARTY.md)).

This project makes the **Pimax Dream Air** work on Linux (Arch / Omarchy, Hyprland) the way Pimax Play does on Windows: one app, one click. The protocol was reverse-engineered from Windows USB captures of our own headset.

**Status: alpha, one tested setup.** Everything here was built and tested on one PC with one headset (see [Tested on](#tested-on)). Expect rough edges on anything else.

> **Before you install.** The setup changes your system, not only your home folder:
> it patches the NVIDIA open kernel module (DKMS) and installs pacman hooks that re-apply the patch after driver updates,
> adds an EDID override and rebuilds the initramfs, installs a udev rule for the headset, and sets `cap_sys_nice` on the Monado service.
> **Take a snapper snapshot first** (the setup also takes one when snapper has a `root` config).
> Read [docs/NVIDIA-UNTESTED.md](docs/NVIDIA-UNTESTED.md) (what happens on other driver versions),
> [docs/UNINSTALL.md](docs/UNINSTALL.md) (how to undo it) and [docs/SECURITY-STATUS.md](docs/SECURITY-STATUS.md) (known open issues).

Names: the app is `omarchyvr-app`, the command line tool `omarchyvr`, the daemon `omarchyvrd`, user data lives in `~/.config/omarchyvr` (and `~/.local/share|state|cache/omarchyvr`). Crates that name Pimax hardware keep their `pimax-` names. The full table is [docs/NAMES.md](docs/NAMES.md).

Everything is reachable from the omarchyvr app (`omarchyvr-app`, starts at login). Play starts VR games, Work starts the VR desktop.

![The omarchyvr app: Home page with Work running (demo data)](docs/gui/home.png)

More pages: [docs/gui](docs/gui).

## Requirements

- A Pimax Dream Air headset (USB id `34a4:0044`). Other Pimax models are not supported.
- Arch Linux or Omarchy with Hyprland. Other Arch-based setups are untested.
- An NVIDIA RTX GPU with `nvidia-open-dkms`. The only tested driver version is **610.57.04**
  (`patches/nvidia/tested-versions.txt`). On any other version the patch is not applied unless you turn on the
  "Allow the patch on untested NVIDIA drivers" switch, and without the patch the headset picture is wrong.
- Steam with SteamVR, installed by you.
- Disk: the installer wants about 40 GB free for the build. The AI and voice downloads come on top and are fetched on first use.

The full table (tested, experimental, unsupported) is in [docs/INSTALL.md](docs/INSTALL.md#requirements).

## Install

    git clone --recursive https://github.com/laughtt/omarchyvr.git
    cd omarchyvr && ./install.sh

The script builds everything and opens the app. The app asks for one password to do the system part. Steps, options and
what the password changes: [docs/INSTALL.md](docs/INSTALL.md).

## Uninstall

The app has **Ready > Uninstall omarchyvr**, or run `./uninstall.sh` in the clone. `./uninstall.sh --dry-run` lists
what would change. Details, what is kept, and how to recover when the display breaks: [docs/UNINSTALL.md](docs/UNINSTALL.md).

![Architecture](docs/img/architecture.png)

The diagram as a page: [docs/diagram/architecture.html](docs/diagram/architecture.html).

## What works (status)

Legend: **works** = used in daily sessions on our PC; **experimental** = runs but rough or not fully verified; **missing** = not there yet.

| Feature | Status | Notes |
|---|---|---|
| Picture 7104x3840, 90 Hz (default) or 72 Hz | works | DisplayPort DRM lease, lens correction fitted to Pimax's data |
| 90 Hz as default | new | start commands match the Windows capture; little long-run testing, 72 Hz stays selectable in Display |
| Head tracking 6DoF (SLAM, Basalt) | works | fused output; loses 6DoF in very fast turns |
| Controllers: buttons, sticks, vibration | works | Oculus Touch profile; pair and unpair from the app |
| Controllers: optical tracking | experimental | rough; occlusion is the main problem |
| Camera hands (Mercury) | experimental | no controllers needed; models are personal use only |
| Room setup + SteamVR chaperone | works | fast setup (look around, floor from the map, forward) |
| Room map viewer | works | one offline HTML page: 3D, floor plan |
| SteamVR mode (driver_monado) | works | Steam updates can break it |
| Monado + xrizer games | works | OpenVR/OpenXR games through Steam/Proton; a game check explains failures |
| WayVR desk with VR screens | works | up to four Hyprland headless outputs |
| Voice ("Max, ...") + AI | works / experimental | Whisper speech-to-text; local or online AI agent is experimental |
| Recording (video + tracking) | works | one folder per take |
| IPD, brightness, render quality presets | works | lenses move from the app; Quality / Balanced / Performance |
| Fast headset restart, pairing | works | restart command copied from Windows (back in about 8 s) |
| Windows capture tooling | works | guided capture session; benchmark of Linux tracking against Pimax |
| Eye tracking | experimental | streams about 119 Hz; accuracy not final; dev-only |
| Speakers in the headset | missing | no sound path found yet (see below) |
| Foveated rendering | missing | needs eye tracking and runtime support |

## Tested on

One PC, one headset. Nothing here is tested on other hardware.

| Part | Ours |
|---|---|
| Distro | Omarchy (Arch based) |
| Kernel | 7.2.5-3-omarchy |
| Compositor | Hyprland 0.56.2 |
| GPU | NVIDIA GeForce RTX 4090 |
| NVIDIA driver | open kernel module 610.57.04, **patched locally** (2Heads1OR + flip fix) |
| CPU | Intel Core i9-14900KF |
| RAM | 62 GB |
| SteamVR | 1789498647 (`bin/version.txt`) |
| Headset | one Pimax Dream Air (SLAM edition) |

## What we control and what we do not

**Ours** (we can fix it, change it, ship it): the Rust tools and the app (`rust/`), the HID protocol library and its allowlist, the Monado fork and the WayVR fork (local commits), the configs and launchers, the docs.

**Not ours** (we only adapt to it):
- Pimax firmware and the headset itself. We never flash it.
- The NVIDIA driver. We carry three local patches that every driver update can break.
- SteamVR, the Steam client and Steam's container (pressure-vessel), Proton.
- Hyprland and Omarchy.

What that means: a Steam or SteamVR update, a new NVIDIA release or a Hyprland change can break VR overnight. We cannot stop that, so the app has a Ready check, a game check and Autofix to find and repair the usual causes, and `omarchyvrd` restarts parts safely. Fixes still need a new build from us.

## Known problems and missing

- **Speakers:** the headset has no USB speaker. Its sound is DisplayPort audio, and on our GPU the leased connector shows no valid audio information. Needs another Windows capture.
- **Eye tracking:** works as a stream but accuracy is not final, and Pimax licensing and tooling make it development-only for now.
- **Foveated rendering:** not built.
- **Controller tracking quality:** optical tracking loses controllers when they leave the cameras' view.
- **Packaging:** `./install.sh` builds from source. An Arch PKGBUILD and an AppImage script exist only as drafts.
- **Security status:** known open issues are listed in [docs/SECURITY-STATUS.md](docs/SECURITY-STATUS.md).
- **NVIDIA patch:** the display needs a patched driver module; no upstream fix.
- **Hand models:** the Mercury models have no licence; they cannot be shipped, only fetched by the user for personal use.

## Roadmap

See [docs/ROADMAP-OMARCHY.md](docs/ROADMAP-OMARCHY.md) (shipping with Omarchy) and [docs/OPEN-SOURCE-PLAN.md](docs/OPEN-SOURCE-PLAN.md) (opening the project).

## Docs index

| File | What |
|---|---|
| [docs/AI.md](docs/AI.md) | The "Max" voice AI agent and its tools |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | How the stack is built: device modules, HID allowlist, checks, privilege boundary |
| [docs/AUDIO.md](docs/AUDIO.md) | Sound in the headset: findings and what is missing |
| [docs/AUTO-MODE.md](docs/AUTO-MODE.md) | Start when the headset is put on, stop when left |
| [docs/EYE.md](docs/EYE.md) | Eye tracking in desk mode |
| [docs/GAMES-CHECK.md](docs/GAMES-CHECK.md) | The VR games check |
| [docs/GUI.md](docs/GUI.md) | The Pimax app, page by page |
| [docs/HANDS.md](docs/HANDS.md) | Camera hand tracking |
| [docs/HEADSET-COMMANDS.md](docs/HEADSET-COMMANDS.md) | Headset commands taken from Windows captures |
| [docs/INSTALL.md](docs/INSTALL.md) | Install steps, what the password changes, NVIDIA policy |
| [docs/UNINSTALL.md](docs/UNINSTALL.md) | Uninstall, what is kept, recovery |
| [docs/SECURITY-STATUS.md](docs/SECURITY-STATUS.md) | Known open security issues |
| [docs/plans/](docs/plans/) | Work plans, starting with [WEEK1-SAFETY.md](docs/plans/WEEK1-SAFETY.md) |
| [CONTRIBUTING.md](CONTRIBUTING.md), [SECURITY.md](SECURITY.md) | How to contribute, how to report a vulnerability |
| [docs/INVENTORY.md](docs/INVENTORY.md) | Inventory of tools and parts |
| [docs/LESSONS.md](docs/LESSONS.md) | Mistakes and pitfalls |
| [docs/MANUAL-STEPS.md](docs/MANUAL-STEPS.md) | Every manual step the app took over |
| [docs/OMARCHYVRD.md](docs/OMARCHYVRD.md) | The daemon that owns the stack |
| [docs/PLAY.md](docs/PLAY.md) | One click to VR: Play and Work |
| [docs/RECORD.md](docs/RECORD.md) | Recording |
| [docs/RENDER-QUALITY.md](docs/RENDER-QUALITY.md) | Quality presets and frame health |
| [docs/RESILIENCE-PLAN.md](docs/RESILIENCE-PLAN.md) | Self-healing plan |
| [docs/ROADMAP-OMARCHY.md](docs/ROADMAP-OMARCHY.md) | Roadmap for Omarchy |
| [docs/ROOM-SETUP.md](docs/ROOM-SETUP.md) | Floor, play area, centre, forward |
| [docs/AUTO-EXPOSURE.md](docs/AUTO-EXPOSURE.md) | Camera auto-exposure: extended range in dim rooms, low-light warning |
| [docs/SCREENS.md](docs/SCREENS.md) | VR screens in Work mode |
| [docs/STEAMVR.md](docs/STEAMVR.md) | SteamVR with the Dream Air |
| [docs/VOICE.md](docs/VOICE.md), [docs/VOICE-DECISIONS.md](docs/VOICE-DECISIONS.md) | Voice control and its decisions |
| [docs/WAYVR-CONTROLS.md](docs/WAYVR-CONTROLS.md) | Controls in the VR desk |
| [docs/WINDOWS-CAPTURE-PLAN.md](docs/WINDOWS-CAPTURE-PLAN.md) | What to capture on Windows next |
| [analysis/](analysis/) | Findings, decoders, research, Windows capture analysis |

## Safety

- The HID allowlist exists and is tested: only bytes captured from Pimax Play on Windows are meant to be sent
  (`rust/pimax-device/devices/<id>/device.toml`). It is not yet applied on every write path; see [docs/SECURITY-STATUS.md](docs/SECURITY-STATUS.md).
- No firmware flashing. No forced display hotplug.
- Never kill a live VR session; installs happen between sessions.
- Personal capture data, serials and calibration of a headset stay out of the public repo. The public export is
  allow-list based (`public-manifest.txt`).

## Report a bug

Open a GitHub issue. Attach the problem report from the app: **Report a problem** writes
`~/.local/state/omarchyvr/reports/omarchyvr-report-<time>.tar.gz` (summary, doctor report, setup checklist, logs; text
goes through a redactor). The file stays on your PC until you share it, so read it before you attach it.
For a security problem, do not open a public issue: see [SECURITY.md](SECURITY.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Vulnerabilities: [SECURITY.md](SECURITY.md).

## Credits and licences

- [Monado](https://monado.freedesktop.org/) (BSL-1.0): our fork carries the Pimax driver.
- [WayVR](https://github.com/wlx-team/wayvr) (GPL-3.0): our fork (github.com/laughtt/wayvr, branch omarchyvr) adds gaze and VR screens; the Monado fork is github.com/laughtt/monado (see patches/README.md).
- [xrizer](https://github.com/Supreeeme/xrizer): OpenVR on top of OpenXR.
- [Basalt](https://gitlab.freedesktop.org/mateosss/basalt) visual-inertial SLAM, via Monado.
- Mercury hand tracking (Monado): models have no licence, personal use only, not redistributed.
- Whisper and the local language model keep their own licences and are downloaded on first use.
- Our code is `MIT OR Apache-2.0` ([LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE)). Docs and data are CC-BY-4.0 ([LICENSE-CC-BY-4.0](LICENSE-CC-BY-4.0)). Third-party parts: [THIRD-PARTY.md](THIRD-PARTY.md).

## For developers

Clone with the forks (Monado and WayVR are git submodules under `forks/`, branch `omarchyvr`):

    git clone --recursive https://github.com/laughtt/omarchyvr.git
    # an existing clone: git submodule update --init --recursive

Enable the leak guard once per clone: `git config core.hooksPath tools/git-hooks` (pre-commit and pre-push run
`tools/leak-scan.py` and refuse private data). The Rust workspace is in `rust/` (`cargo test` in that folder). Build from a clean checkout of committed HEAD. The Monado build must keep its feature markers (`PIMAX_SLAM`, `PIMAX_ANCHOR`, `PIMAX_EYE`, `PIMAX_CTRL_TRACK` and more; the full list is in [ai/CLAUDE.md](ai/CLAUDE.md)). Details: [docs/INVENTORY.md](docs/INVENTORY.md).

## For AI agents

Start with [AGENTS.md](AGENTS.md); the binding rules are in [ai/CLAUDE.md](ai/CLAUDE.md).
