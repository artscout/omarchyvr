# omarchyvr: Pimax Dream Air VR on Linux (Omarchy / Arch)

**omarchyvr is an unofficial community project. It is not affiliated with, made by, or endorsed by Pimax or Omarchy.** "Pimax" and "Dream Air" are trademarks of Pimax and are used only to say which hardware this works with. No warranty: the tools talk to the headset over USB and change system settings; use at your own risk. Licence: code MIT OR Apache-2.0, docs and data CC-BY-4.0 (see NOTICE, THIRD-PARTY.md).

Binary, crate and config names still use the `pimax` prefix (`pimax-setup`, `~/.config/pimax`); they will be renamed later.

Unofficial. This project makes the **Pimax Dream Air** work on Linux (Arch / Omarchy, Hyprland) the way Pimax Play does on Windows: **one app, one click, everything part of one whole.** It is not made or endorsed by Pimax. The protocol was reverse-engineered from Windows USB captures of our own headset.

The user never needs a terminal: everything is reachable from the Pimax app (`pimax-setup`, starts at login). Play starts VR games, Work starts the VR desktop.

![Architecture](docs/img/architecture.png)

The diagram as a page: [docs/diagram/architecture.html](docs/diagram/architecture.html).

## What works (status)

Legend: **works** = used in daily sessions on our PC; **experimental** = runs but rough or not fully verified; **missing** = not there yet.

| Feature | Status | Notes |
|---|---|---|
| Picture 7104x3840, 72 Hz | works | DisplayPort DRM lease, lens correction fitted to Pimax's data |
| 90 Hz | experimental | start commands match the Windows capture; little long-run testing |
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

What that means: a Steam or SteamVR update, a new NVIDIA release or a Hyprland change can break VR overnight. We cannot stop that, so the app has a Ready check, a game check and Autofix to find and repair the usual causes, and `pimaxd` restarts parts safely. Fixes still need a new build from us.

## Known problems and missing

- **Speakers:** the headset has no USB speaker. Its sound is DisplayPort audio, and on our GPU the leased connector shows no valid audio information. Needs another Windows capture.
- **Eye tracking:** works as a stream but accuracy is not final, and Pimax licensing and tooling make it development-only for now.
- **Foveated rendering:** not built.
- **Controller tracking quality:** optical tracking loses controllers when they leave the cameras' view.
- **Packaging:** no installer for other people yet (an Arch PKGBUILD and an AppImage script exist as drafts).
- **NVIDIA patch:** the display needs a patched driver module; no upstream fix.
- **Hand models:** the Mercury models have no licence; they cannot be shipped, only fetched by the user for personal use.
- **No LICENSE file yet** in this repo; see the open-source plan.

## Roadmap

See [docs/ROADMAP-OMARCHY.md](docs/ROADMAP-OMARCHY.md) (shipping with Omarchy) and [docs/OPEN-SOURCE-PLAN.md](docs/OPEN-SOURCE-PLAN.md) (opening the project).

## For AI agents

Start with [AGENTS.md](AGENTS.md) and [docs/AI-KNOWLEDGE.md](docs/AI-KNOWLEDGE.md). The binding rules are in [.claude/CLAUDE.md](.claude/CLAUDE.md).

## Docs index

| File | What |
|---|---|
| [docs/AI.md](docs/AI.md) | The "Max" voice AI agent and its tools |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Design for the open-source release |
| [docs/AUDIO.md](docs/AUDIO.md) | Sound in the headset: findings and what is missing |
| [docs/AUTO-MODE.md](docs/AUTO-MODE.md) | Start when the headset is put on, stop when left |
| [docs/EYE.md](docs/EYE.md) | Eye tracking in desk mode |
| [docs/GAMES-CHECK.md](docs/GAMES-CHECK.md) | The VR games check |
| [docs/GUI.md](docs/GUI.md) | The Pimax app, page by page |
| [docs/HANDS.md](docs/HANDS.md) | Camera hand tracking |
| [docs/HEADSET-COMMANDS.md](docs/HEADSET-COMMANDS.md) | Headset commands taken from Windows captures |
| [docs/INVENTORY.md](docs/INVENTORY.md) | Inventory of tools and parts |
| [docs/LESSONS.md](docs/LESSONS.md) | Mistakes and pitfalls |
| [docs/MANUAL-STEPS.md](docs/MANUAL-STEPS.md) | Every manual step the app took over |
| [docs/PIMAXD.md](docs/PIMAXD.md) | The daemon that owns the stack |
| [docs/PLAY.md](docs/PLAY.md) | One click to VR: Play and Work |
| [docs/RECORD.md](docs/RECORD.md) | Recording |
| [docs/RENDER-QUALITY.md](docs/RENDER-QUALITY.md) | Quality presets and frame health |
| [docs/RESILIENCE-PLAN.md](docs/RESILIENCE-PLAN.md) | Self-healing plan |
| [docs/ROADMAP-OMARCHY.md](docs/ROADMAP-OMARCHY.md) | Roadmap for Omarchy |
| [docs/ROOM-SETUP.md](docs/ROOM-SETUP.md) | Floor, play area, centre, forward |
| [docs/SCREENS.md](docs/SCREENS.md) | VR screens in Work mode |
| [docs/STEAMVR.md](docs/STEAMVR.md) | SteamVR with the Dream Air |
| [docs/VOICE.md](docs/VOICE.md), [docs/VOICE-DECISIONS.md](docs/VOICE-DECISIONS.md) | Voice control and its decisions |
| [docs/WAYVR-CONTROLS.md](docs/WAYVR-CONTROLS.md) | Controls in the VR desk |
| [docs/WINDOWS-CAPTURE-PLAN.md](docs/WINDOWS-CAPTURE-PLAN.md) | What to capture on Windows next |
| [STATUS.md](STATUS.md), [HISTORY.md](HISTORY.md), [NEXT-STEPS.md](NEXT-STEPS.md) | Older status, chronology (early notes, partly out of date), next steps |
| [analysis/](analysis/) | Findings, decoders, research, Windows capture analysis |

## Safety

- HID bytes are sent only if they were captured from Pimax Play on Windows (allowlist in code).
- No firmware flashing. No forced display hotplug.
- Never kill a live VR session; installs happen between sessions.
- Capture data, serials and calibration of a headset are personal: keep this repo private if they are in it.

## Credits and licences

- [Monado](https://monado.freedesktop.org/) (BSL-1.0): our fork carries the Pimax driver.
- [WayVR](https://github.com/wlx-team/wayvr) (GPL-3.0): our fork (github.com/laughtt/wayvr, branch omarchyvr) adds gaze and VR screens; the Monado fork is github.com/laughtt/monado (see patches/README.md).
- [xrizer](https://github.com/Supreeeme/xrizer): OpenVR on top of OpenXR.
- [Basalt](https://gitlab.freedesktop.org/mateosss/basalt) visual-inertial SLAM, via Monado.
- Mercury hand tracking (Monado): models have no licence, personal use only, not redistributed.
- Whisper and the local language model keep their own licences and are downloaded on first use.
- Our own Rust code is declared `MIT OR Apache-2.0` in `Cargo.toml`; LICENSE files are still to be added.

## For developers

The Rust workspace is in `rust/` (`cargo test` in that folder). Build from a clean checkout of committed HEAD. The Monado fork must keep its markers (`PIMAX_SLAM`, `PIMAX_ANCHOR`, `PIMAX_EYE`, `PIMAX_CTRL_TRACK`). Details and rules: [.claude/CLAUDE.md](.claude/CLAUDE.md), [docs/INVENTORY.md](docs/INVENTORY.md).
