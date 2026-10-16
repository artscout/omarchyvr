# VR home options on Linux with Monado (research, 2026-10-15)

Question: what can serve as a "VR home" (land here when the headset goes on, launch Steam/VR games), like SteamVR Home on Windows, on Monado + NVIDIA + Wayland with our Pimax Dream Air driver?

## Findings

1. **Monado has no home or launcher.** Monado is a runtime only. The vronlinux guide says "Monado has no launcher of its own, so after connecting a headset the displays stay black until you start an XR app" ([vronlinux Envision page](https://vronlinux.org/docs/fossvr/envision/)). A Khronos forum post says Monado's developers keep the system/dashboard UI minimal, and that a dashboard would need a vendor extension with one app allowed at a time ([Khronos forum](https://community.khronos.org/t/runtime-intercepting/107874), not dated). Monado's about-runtimes page does not mention a home or dashboard ([Monado webpage](https://gitlab.freedesktop.org/monado/webpage/-/blob/master/about-runtimes.md), fetch checked). No 2026 Monado home or demo found.

2. **SteamVR Home on Linux with our headset: not a path today.** Our repo:
   - `analysis/customheadsetopenvr.md:48`: CustomHeadsetOpenVR builds on Linux and "can work with SteamVR and Monado", but "none of the headsets work on Linux". The SLAM path uses Windows APIs and the closed Windows-only PVR client. Line 45: the compositor shader is D3D11 and Windows-only.
   - `docs/AUTO-MODE.md:63`: `game_runtime = steamvr` is "phase 1 (static)". Auto mode starts nothing for it and only notifies.
   - `docs/LESSONS.md:44` (row 20): OpenVR games use xrizer (`vr_runtime_switch.sh xrizer`). SteamVR is installed on this box, but SteamVR rewrites `openvrpaths.vrpath` and the fix is `chmod 444`.
   - Conclusion: SteamVR can be installed and run, but nothing in the repo makes it drive the Dream Air on Linux. SteamVR Home would need a Linux SteamVR driver for the headset. Not viable now.

3. **Open-source launchers and homes on Monado**

   | Option | What it gives | Launches Steam games? | Monado / NVIDIA / Wayland | Effort | Risks |
   |---|---|---|---|---|---|
   | **WayVR dash (our fork, `~/src/wayvr`)** | In-VR dashboard with Home, Apps, Games and Monado tabs, plus desktop capture and app panels | **Yes.** `dash-frontend/src/util/steam_utils.rs` reads `libraryfolders.vdf` and `appmanifest_*.acf`, and `launch()` runs `steam://rungameid/<id>` (lines 128-130). Games tab: `dash-frontend/src/tab/games.rs` | Monado: yes, the README lists Monado/WiVRn/SteamVR. Wayland: supported; X11 is slower. NVIDIA: not mentioned upstream; our desk session already runs it on this box. | Low: already in our desk session. Needs a Play entry point, not a new app | Steam-launch path (`steam://`) may skip the launch options (see note below); "Steam Frame support is experimental"; 46 open issues upstream |
   | **Stardust XR** | Spatial display server. Demo shows a hexagon-grid app launcher, Wayland app panels, and a CRT-style XR shell | Not mentioned on the site | Monado not mentioned. Platforms not specified. | High: a whole compositor/shell, not a drop-in | Not a Steam launcher; no release info on the site; could replace WayVR's role rather than add to it |
   | **Overte** | Open-source social VR world platform (Apache 2.0), with desktop and VR use | No. It is a virtual-world client, not a game launcher | No OpenXR/Monado/SteamVR mention on the project page; no evidence found | High | Wrong category; not a home for Steam games |
   | **Envision** | GUI that sets up and runs Monado/WiVRn profiles (incl. system Monado + xrizer); suggests starting WayVR via its Plugins menu | No game library. It runs the runtime and WayVR, not Steam | Monado: yes. NVIDIA/Wayland: not verified. | Low to medium, if we want a setup GUI | Marked "highly experimental" (Fedora); often breaks after updates (NixOS wiki); not a home |
   | **xrdesktop / other** | Desktop-in-VR | No | Not checked | — | Not a home |

   Sources: [WayVR repo](https://github.com/wayvr-org/wayvr); [WayVR local source](/home/user/src/wayvr/dash-frontend/src/util/steam_utils.rs); [Stardust XR](https://stardustxr.org); [Overte repo](https://github.com/Overte-org/overte); [Envision, vronlinux](https://vronlinux.org/docs/fossvr/envision/); [Envision, FOSDEM 2025](https://fosdem.org/schedule/event/fosdem-2025-5976-linux-vr-beginner-s-guide-on-how-to-join-events-in-virtual-reality-from-ubuntu-using-envision-and-monado-an-openxr-alternative-to-steamvr); [Envision, Fedora](https://packages.fedoraproject.org/pkgs/envision/envision/index.html).

4. **How games are launched from VR on Linux today**
   - **Steam library:** WayVR's Games tab lists installed Steam apps from the local Steam manifests and launches them with `steam://rungameid/<id>` (`steam_utils.rs:128-130`). Its own code has a check for a `SteamLaunch` argument (`steam_utils.rs:182-190`). The launch options are the part to verify live.
   - **OpenXR games:** the Steam launch option `PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1 %command%` lets the Steam Linux Runtime see the OpenXR runtime ([vronlinux Monado guide](https://vronlinux.org/docs/fossvr/monado/), via the WayVR search snippet). Our repo's `docs/LESSONS.md:47` (row 23) says `-openxr` fails for Dungeons of Eternity; use the SteamVR/OpenVR launch mode instead.
   - **OpenVR games:** xrizer is an OpenVR-to-OpenXR layer with Monado support ([xrizer repo](https://github.com/Supreeeme/xrizer)). It is "currently immature", has no compatibility list, and excludes overlay and background apps. Setup is `openvrpaths.vrpath` (our `vr_runtime_switch.sh xrizer`) or `VR_OVERRIDE`. Our repo: Dungeons of Eternity works (`docs/MANUAL-STEPS.md:201`).
   - **Steam big-picture overlay in WayVR:** not verified. Nothing found in the sources.

## Recommendation

Use **WayVR's dashboard as the VR home** and add a **Play** button that lands the user in it: the desk session already starts WayVR, so the home is already on screen when the headset goes on. Make Play open the Games tab (or a Play entry point) and call `steam://rungameid/<appid>` through the same code as `steam_utils::launch`, with the per-game launch options from `pimax vr steam-options` for OpenXR or OpenVR. Verify first: that `steam://` launches from inside a WayVR session pass the launch options and reach xrizer or Monado; and that a game started this way does not stop the desk session (`docs/AUTO-MODE.md:33`). Skip SteamVR Home (no Linux driver for the Dream Air) and Stardust/Overte (wrong scope or too heavy for this step).

## Gaps

- No Monado home or demo exists as of the sources checked (no 2026 release found).
- The Stardust XR Steam-launch status, the Overte OpenXR status, and the WayVR NVIDIA status are not verified.
- The xrizer 2026 status is only the README; no release date was shown.
- The WayVR-in-Steam overlay and the big-picture overlay in WayVR were not found in sources.
