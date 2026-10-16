# Roadmap: ship the Pimax Dream Air stack with Omarchy

Date: 2026-10-15. Written from the repo, the local forks and the Omarchy manual. Nothing was changed or installed to write it.

## 1. Verdict

**No. It is not ready to ship as an Omarchy package today.** It works well on this one machine. It is not yet a product.

Blockers (details in section 4):

1. **Monado and WayVR are not packaged.** The only PKGBUILD (`packaging/arch/PKGBUILD`, `pimax-linux-git`) builds the Rust tools and the app. The Monado and WayVR forks stay user builds in `~/src`.
2. **NVIDIA needs a patched kernel module.** The stack relies on our `merged_head_sync` patch (flip-hang fix) and an EDID override. Stock `nvidia-open` does not have them. This is the hardest one.
3. **Hardcoded paths.** `~/src/monado-eye/build`, `~/src/wayvr`, `~/src/target-desk`, `~/src/venv-voice`, the vendored onnxruntime and the app's Builds page all assume this machine.
4. **Hand models have no licence.** They cannot be shipped. Hands must be an optional user download.
5. **Games are not proven.** No SteamVR run, and one OpenVR game through xrizer (Dungeons of Eternity) only. There is no VR home for Play yet.
6. Also: known failing tests, no LICENSE file in the repo, forks never upstreamed, a reverse-engineered HID protocol with no Pimax permission.

Good news: the app, the launcher, omarchyvrd (self-healing), Work mode (WayVR desk), voice and the AI tab already exist and are GUI-driven.

## 2. What "install = everything works" means

One install, then the Pimax app opens by itself at login and walks the user through the rest. No command line for the user (project rule, section 1).

| Piece | Delivered by | Root step | Notes |
|---|---|---|---|
| Pimax app `omarchyvr-app` + autostart | package `pimax-linux` | none (autostart file in `/etc/xdg/autostart`) | exists in the PKGBUILD, autostart to check |
| Rust tools (`omarchyvr`, `omarchyvr-voice`, `omarchyvr-record`, `omarchyvrd`, ...) | same package | none | whole workspace, installed together |
| Our Monado fork (Pimax driver, SLAM, eye, controller tracking, anchor, recorder tap, camera hands) | package `monado-pimax` | `cap_sys_nice` on `monado-service` | one build, all markers; Basalt is a build dependency |
| WayVR fork (eye-gaze, screens, hands pointer) | package `wayvr-pimax` | none | must not conflict with Arch/AUR `wayvr` |
| OpenVR bridge | package `xrizer` (AUR or ours) | none | GPL-3.0-or-later |
| udev rules (HID 34a4, USB, 2104:0561) | `pimax-linux` | udev reload (package hook) | file exists in `packaging` |
| setcap for `monado-service` | package `post_install`/`post_upgrade` hook, or file capabilities in the package | none for the user | fallback: the app's Fix button via pkexec |
| NVIDIA: `nvidia-open` driver, non-desktop EDID, `nvidia-modeset` config, initramfs | `pimax-nvidia` package plus a pacman hook | one pkexec prompt, then a reboot | see blocker 2 |
| Kernel params | none needed beyond the NVIDIA ones; the app's Checks page verifies `nvidia-drm.modeset=1` | pkexec | check list lives in `pimax-device` |
| Hyprland integration | app writes a `conf.d` snippet and window rules; DP-1 leased as non-desktop | none | do not edit `~/.config/hypr` by hand |
| Voice (Whisper) and local AI (llama.cpp + Qwen) | app downloads models on first use (AI tab, Install button) | none | models are downloads, not in the package |
| First-run setup | the app's Onboarding page (exists), extended into a full wizard | pkexec steps inside it | see phase 1 |

Definition: after `install`, a user with an NVIDIA GPU and the headset on DP presses **Play** or **Work** and it works, after at most one reboot.

## 3. Current state inventory

### 3.1 Done (in the repo, 327 commits)

| Area | State | Where |
|---|---|---|
| Rust workspace | 30+ crates: device protocol, tools, config, GUI core, setup app, voice, AI, hands, eye, SLAM glue, ctrl tracking, anchor, room map, record, passthrough, lens, doctor | `rust/`, `docs/INVENTORY.md` |
| App | Home (Play and Work), Setup (Checks, Builds, Autofix, Onboarding), Room, AI, System (omarchyvrd) pages | `docs/GUI.md`, `docs/PLAY.md` |
| Self-healing | omarchyvrd with components, GPU gate, careful restarts | `docs/PIMAXD.md`, `docs/RESILIENCE-PLAN.md` |
| Work (desk) | WayVR desk, up to 4 screens, eye gaze, hands before controllers, voice commands | `docs/SCREENS.md`, `docs/VOICE.md`, `docs/EYE.md` |
| Voice and AI | wake word "Max", offline matcher, local model, remote two-stage | `docs/VOICE.md`, `docs/AI.md` |
| Hands | Mercury camera hands as `XR_EXT_hand_tracking` | `docs/HANDS.md` |
| Packaging start | Arch PKGBUILD for the Rust side, AppImage script | `packaging/` |
| Games (partial) | xrizer built; Dungeons of Eternity works through SteamVR mode (OpenVR to xrizer to Monado) | `analysis/steamvr-games.md`, `docs/LESSONS.md` rows 20, 23 |

### 3.2 Forks and local-only work

| Repo | Branch | State | Upstream |
|---|---|---|---|
| `~/src/monado-eye` (Monado) | `desk` (merged slam, eye, hands) | local commits only (rule: never push). 15 recent commits cover Pimax driver, controllers, anchor, recorder tap, camera hands, HID-loss handling | not upstreamed; large Pimax driver + Rust library hook (`monado-opt-in-rust-libpimax.patch`) |
| `~/src/wayvr` | `eye-gaze` | local commits only: gaze target, desk-screens, screen-nudge, bounded shutdown | not upstreamed |
| `~/src/CustomHeadsetOpenVR` | `linux-dreamair` | local only, push disabled on purpose (GPL-2.0) | no |
| `~/src/xrizer` | upstream clone | built from Supreeeme/xrizer | upstream |
| NVIDIA module | `~/src/nvidia-610.57.04-flipfix`, `open-gpu-kernel-modules` | our patches applied by a pacman hook | a draft comment for NVIDIA PR 1262 exists (`analysis/nvidia-pr1262-comment-draft.md`); not merged |
| Basalt, onnxruntime 1.20.1, whisper.cpp, sherpa venvs | `~/src/...` | vendored by hand | upstream projects |

About 15 other build directories sit in `~/src` (`monado`, `monado-slam`, `monado-ht`, `monado-lens`, `monado-cam` ...). Rule 3 of the project says there must be only one build. They are leftovers to remove.

### 3.3 Hardcoded to this machine

| Item | Example | Fix |
|---|---|---|
| Monado build location | `~/src/monado-eye/build` (17 mentions in Rust sources), `~/src/monado/build` (6) | ask the package: `/usr/lib/monado-pimax/...`, the app reads one path from config |
| WayVR location | `~/src/wayvr` | `/usr/bin/wayvr-pimax` |
| Cargo targets | `~/src/target-desk`, `target-eye`, ... | not needed after packaging |
| Voice Python venv | `~/src/venv-voice` (5) | Rust/ONNX voice, or a venv the app creates under `~/.local/share/omarchyvr` |
| ONNX Runtime | vendored `~/src/onnxruntime-1.20.1` | AUR/Arch `onnxruntime` package or a pinned download |
| Calibration | `/home/user/pimax-re/analysis/headset-calibration.xml` | read from the headset or from `/usr/share/omarchyvr` |
| User path in tests and docs | `/home/user` | use `$HOME` |
| PKGBUILD source | `git+file:///home/user/...` example | tagged tarball from GitHub |
| NVIDIA PCI id in the modeset config | `0000:01:00.0` | detect it |
| Connector | `DP-1` | the app already has `connector_auto`; keep it |

### 3.4 Tests

Known failing or fragile (the project has run them as "pre-existing"):

| Test | Problem | Action |
|---|---|---|
| `omarchyvr-app` e2e: "the UI thread read files during frames" (`tests/common/mod.rs:257`) | a page reads files on the UI thread | move the read to a worker, or cache it |
| `omarchyvr-tools` `calib_offline_decodes_the_real_windows_blob` (`tests/cli.rs:100`) | fails; CRLF in the calibration XML is one suspect | normalise line endings; add a small fixture |
| CRLF in calibration XML | Windows capture has CRLF | parser trims `\r` |
| `pimax-lens-difftest` | fails | find the cause; mark the golden |

Clean packaging builds need `cargo test --workspace` green. Goldens must pass.

## 4. Blockers for distribution

### 4.1 Licences

| Piece | Licence | Consequence |
|---|---|---|
| Monado | BSL-1.0 | free to ship; our fork stays BSL-1.0 and must keep notices |
| WayVR | GPL-3.0 (`~/src/wayvr/LICENSE`) | free to ship; our fork must publish source (GPL) |
| xrizer | GPL-3.0-or-later | free to ship |
| CustomHeadsetOpenVR | GPL-2.0 | only if we ever use it; do not mix with GPL-3 code without checking |
| Our Rust workspace | `MIT OR Apache-2.0` in `Cargo.toml`, **no LICENSE file** | add LICENSE files before any release |
| Mercury hand models | **none** (no LICENSE file; GitLab reports none) | **cannot be redistributed or bundled.** Make hands an optional extra: the app offers a "Download hand models" button that fetches them on the user's machine from the upstream source (needs git-lfs), with a plain note. Long term: ask the author for a licence, or train or pick a licensed model |
| Basalt | BSD-3 | free |
| Whisper and Qwen models | their own licences | download on first use, not bundled |
| Pimax firmware or blobs | none in the repo | keep it that way; "no firmware flashing" rule stays |
| Reverse-engineered HID protocol | taken from Windows captures | legal risk is low in many places but not zero. Ask a lawyer or at least state "unofficial, not affiliated with Pimax". The name "Pimax" in the package name is a trademark question: prefer `dreamair-linux`-style names or ask Pimax |

### 4.2 Building forks as packages

Needed PKGBUILDs (own repo, later AUR):

| Package | Content | Hard parts |
|---|---|---|
| `monado-pimax` | Monado fork, one build with every marker; must provide `monado-service` and the OpenXR runtime json; conflicts or provides `monado` | needs the Rust `libpimax` linked in (`PIMAX_CAM=ON`, `XRT_BUILD_DRIVER_PIMAX=ON`, `XRT_HAVE_SYSTEM_CJSON=OFF`); Basalt, ONNX Runtime, cargo at build time; `check` step greps all markers (`PIMAX_SLAM`, `PIMAX_ANCHOR`, `PIMAX_EYE`, `PIMAX_CTRL_TRACK`, `held from the tracker`) |
| `wayvr-pimax` | WayVR eye-gaze fork | Arch has `wayvr` in AUR; use `conflicts`/`provides` |
| `pimax-linux` | Rust tools + app (exists as `pimax-linux-git`) | fix paths, tag releases, ship LICENSE |
| `pimax-nvidia` | patch set, EDID file, hook | see 4.3 |
| `xrizer` | OpenVR bridge | exists upstream; check AUR |

Sources must be public for a package. Monado and WayVR commits are local-only today. Before the first PKGBUILD can work, the forks need public repos (a decision for the user; the project rule says "never push" for Monado and WayVR, so this needs an explicit change).

### 4.3 NVIDIA

- Our stack needs the **`merged_head_sync` patch** to the open kernel module and an **EDID override** that marks the panel as non-desktop so Hyprland leases it (`docs/MANUAL-STEPS.md` 1.1; `analysis/nvidia-flip-hang.md`).
- A package that patches the driver with DKMS is possible (`nvidia-open-dkms` plus a patch hook). It breaks on every NVIDIA update and is risky for a default install.
- Best path: **get the patch accepted by NVIDIA** (`open-gpu-kernel-modules` PR 1262 draft is in `analysis/`), then Omarchy needs nothing special for the kernel.
- Until then: ship `pimax-nvidia` as an **opt-in** package. The app's Setup page checks "patch loaded?" and offers the install via pkexec.
- Only NVIDIA is covered. AMD and Intel are untested and out of scope for the first release; say so.

### 4.4 setcap versus pkexec

- A package can set the file capability itself (`setcap` in the `post_install` and `post_upgrade` hook, or `filecaps` in the package). That is clean because the package owns `monado-service` and the capability returns after every upgrade.
- The app's **Fix** button (pkexec) stays as a safety net for developer builds.
- Both together: package hook first, pkexec second.

### 4.5 How Omarchy takes packages (researched)

- The official manual says Omarchy installs from the **Arch repositories** and the **AUR** (menu Install > Package and Install > AUR, or `omarchy pkg add`). It warns that the AUR is not vetted by Arch. Source: <https://learn.omacom.io/2/the-omarchy-manual/66/other-packages>.
- The manual also says Omarchy builds some packages itself and hosts them in a separate repository (AUR packages it needs by default, the Chromium fork). Source: same page (search summary, not quoted on the page text).
- A third-party summary says Omarchy ships as the packages `omarchy` and `omarchy-settings` and that the PKGBUILDs live in a repo called `omarchy-pkgs`. This is not official; verify before relying on it: <https://instagit.com/omacom/omarchy/how-is-omarchy-structured-as-arch-linux-packages.md>.
- Conclusion: **nothing "installs by default" unless the Omarchy maintainers add it to the base install.** For a niche NVIDIA-plus-Pimax stack that is unlikely. Realistic path: our own repo and AUR packages first, installed from the Omarchy menu (Install > AUR), later a PR to Omarchy for an optional "Install > VR > Pimax Dream Air" entry. Ask the Omarchy maintainers (Discord or GitHub discussions) before any PR. I did not read the `basecamp/omarchy` GitHub repo itself; check it for the real install-script layout before the submission phase.

## 5. Games

### 5.1 Monado and OpenXR native

| Question | Status |
|---|---|
| What works now | WayVR desk (Work). Play starts Monado with the base services, for Steam (xrizer) and OpenXR games, and starts nothing else |
| VR home or launcher | **None.** Monado has no home of its own. Research (`analysis/research/vr-home-options.md`) recommends the **WayVR dashboard** (Home, Apps, Games, Monado tabs; Games lists Steam apps and launches `steam://rungameid/<id>`). The user has not chosen yet |
| OpenXR native games | work through the active runtime json. Steam launch option `XR_RUNTIME_JSON=... %command%`, or `PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1 %command%` for the Steam Linux Runtime ([vronlinux Monado guide](https://vronlinux.org/docs/fossvr/monado/)) |
| Proton, OpenXR mode | works with the launch option `PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1 %command%` (LESSONS rows 23, 27); OpenVR mode stays the default |
| OpenVR games | xrizer translates OpenVR to OpenXR on Monado. Upstream calls it immature, with no compatibility list ([xrizer](https://github.com/Supreeeme/xrizer)). Overlays and background apps are not supported |
| 32-bit games | xrizer 32-bit build skipped (needs rustup target) |
| What is missing | a Play entry point in WayVR, per-game launch options from the app, a compatibility list kept by us, controller bindings checked per game, verifying that `steam://` launched from WayVR keeps launch options |

### 5.2 SteamVR (simulation, to test)

Facts from the repo:

- `omarchyvr vr steam-options` prints only the Steam launch options line for Monado: `PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1 [XR_RUNTIME_JSON=<runtime json>] %command%` (`rust/omarchyvr-tools/src/vr/config.rs`, `steam_options`). Since pressure-vessel 0.20260925 the container shares the Monado IPC socket only with that variable (LESSONS row 27). It does not start SteamVR.
- Auto mode `game_runtime = steamvr` is "phase 1 (static)": nothing is started, it only notifies (`docs/AUTO-MODE.md:63`).
- The design for real SteamVR is in `analysis/steamvr-native-linux.md`: Monado's SteamVR plugin `driver_monado.so` (already built from our branch) for tracking, plus the CustomHeadsetOpenVR fork for display. Known gaps there: no EDID properties, no touch-controller bindings, no eye-gaze. The SteamVR Linux compositor can use the Wayland DRM lease, so the display path looks possible, but **nothing has been run**.
- Two ways to run OpenVR games on this headset:

| Way | What drives the headset | Status |
|---|---|---|
| A. xrizer on Monado | our Monado | works for one game |
| B. SteamVR with Monado plugin | SteamVR's vrserver loads our driver | design only; stage 1 (static pose, display path) built per docs, untested live |

SteamVR also rewrites `openvrpaths.vrpath`; the fix used today is making the file read-only (LESSONS row 20). The app must manage this.

**Test plan** (done from the app, in the headset; each test is one button that starts, talks in VR and records):

| # | Test | Game (free or cheap; pick what the user owns) | Expected | Record |
|---|---|---|---|---|
| T1 | OpenVR game through xrizer | Dungeons of Eternity (already works) | starts, head tracks, controllers move, no Xid | FPS and late frames (`PIMAX_PERF_FILE`), screenshot of each eye, log |
| T2 | Free OpenVR Proton game | `The Lab` (Valve, free) or any free OpenVR demo the user owns | menu shows, laser pointer works, grab works | same as T1, plus button map notes |
| T3 | OpenXR native | Monado's `hello_xr` (sample), then a free OpenXR game such as `Vivecraft` (Minecraft Java, needs the licence) or any OpenXR title the user has | stereo correct, no ghosting, 72 Hz | same |
| T4 | SteamVR runtime stage 1 | SteamVR "Room setup" (no game) | headset appears, display lit, no pose yet | vrserver and vrcompositor logs, a photo of the display state |
| T5 | SteamVR runtime stage 2 | `The Lab` first level | head tracking from our driver, controller buttons (after bindings patch) | logs, FPS |
| T6 | Stop and restart | any | session ends clean, no leftover `monado-service`, cap still set | omarchyvrd verdict |

Game picks are suggestions; the user should confirm which titles he owns. The app should show a "Games test" page that lists the tests, runs each, and saves a report (HTML) the agents can read.

## 6. Roadmap

Legend: **A** = agents can do it. **U** = needs the user (headset test or decision).

### Phase 0: clean the base (A)

| Milestone | Done when |
|---|---|
| Fix 4 failing tests | `cargo test --workspace` green, goldens unchanged |
| Remove hardcoded paths | no `/home/user` or `~/src` in Rust sources (grep clean); paths come from one config |
| Add LICENSE files | `LICENSE-MIT`, `LICENSE-APACHE` in repo; third-party notice file |
| Remove stale builds | the app's Builds page lists one Monado build |

### Phase 1: packages and first-run wizard (A, then U to test)

| Milestone | Done when |
|---|---|
| `pimax-linux` PKGBUILD builds from a tag in a clean chroot | `makepkg` in a clean chroot succeeds; `namcap` has no errors |
| `monado-pimax` PKGBUILD | all markers found by the package `check()`; no `~/src` needed |
| `wayvr-pimax` PKGBUILD | desk session starts from `/usr/bin` |
| Package hooks | udev reload, setcap on `monado-service` survive `pacman -U` twice |
| Onboarding wizard in the app | on a fresh user account: finds the headset, runs Checks, offers each fix with pkexec, ends with a green Play button. Screenshot looked at |
| Test on a clean install | **U**: a second machine or a fresh Omarchy VM with GPU passthrough, or a fresh user plus a clean chroot install on this machine |
| Public source for forks | **U** decision first (section 7) |

### Phase 2: game path, OpenXR native and xrizer (A, U)

| Milestone | Done when |
|---|---|
| Play opens the VR home (WayVR dash Games tab) | **U** choose home; then Play starts it, saved settings, start-argv recorded |
| Per-game launch options from the app | the Games page shows a button "set launch options" or the app sets them for the user (Steam `localconfig.vdf` edit, with backup) |
| xrizer packaged and runtime switch automated | `openvrpaths.vrpath` managed by the app, no manual file |
| Tests T1 to T3 | **U** in headset; reports saved |
| Compatibility list | a table in `docs/GAMES.md` with game, result, launch mode, notes |

### Phase 3: SteamVR test and integration (A, U)

| Milestone | Done when |
|---|---|
| T4 SteamVR stage 1 | **U** headset test; display lit through SteamVR, logs saved |
| Patch Monado plugin: EDID props, touch bindings | A; unit tested; build keeps all markers |
| T5 SteamVR stage 2 | **U**; head and controllers in a real game |
| Decision: keep SteamVR or drop it | **U** with the data; if xrizer covers the games, SteamVR may stay "experimental" |
| App switch "Game runtime: Monado+xrizer / SteamVR" | the setting is saved, shared with the CLI, applied by Play |

### Phase 4: Play button as VR home (A, U)

| Milestone | Done when |
|---|---|
| Play starts home, a game launches from inside the headset | **U** confirms in the headset; no desk session stop |
| Voice: "start game X" | voice catalogue entry plus audit phrases |
| Docs `docs/PLAY.md` updated | yes |

### Phase 5: Omarchy submission (A, U)

| Milestone | Done when |
|---|---|
| Own repo (pacman repo on GitHub Pages or similar) with signed packages | Omarchy can add it as a repo; install by menu works |
| AUR packages published | `monado-pimax`, `wayvr-pimax`, `pimax-linux` |
| Ask Omarchy maintainers | **U** opens a discussion or issue; outcome noted. Ask for an optional entry only, not default |
| NVIDIA patch upstream push | **U** posts (draft exists); or keeps `pimax-nvidia` opt-in |

### Phase 6: docs and website (A)

| Milestone | Done when |
|---|---|
| User guide: install, first run, troubleshooting (GUI words only) | in repo and on a web page |
| Hardware and game compatibility pages | from Phase 2 and 3 tables |
| Honest status page | "unofficial, NVIDIA only, tested on one machine" until more testers |

## 7. Open decisions for the user

| # | Decision | Recommended answer |
|---|---|---|
| 1 | Publish the Monado and WayVR forks on GitHub (needed for packages)? | Yes, as public forks with clean history; set a new rule for them |
| 2 | VR home for Play | WayVR dashboard (Games tab); lowest effort, already in the desk session |
| 3 | Package name (Pimax trademark) | `dreamair-linux` family, say "unofficial" in the text |
| 4 | Hand models | optional download button; ask the author for a licence meanwhile |
| 5 | NVIDIA patch | push upstream, keep `pimax-nvidia` opt-in until accepted |
| 6 | SteamVR: invest or xrizer only? | run T1 to T5 first; decide after the data |
| 7 | Where to start: own repo or AUR? | own pacman repo first (full control), AUR after it is stable |
| 8 | Test machine | one clean install (VM or second disk) to prove the wizard |
| 9 | Which games do you own for the test plan? | confirm the list in section 5.2 |

## Sources

- Omarchy manual, other packages: <https://learn.omacom.io/2/the-omarchy-manual/66/other-packages>
- Omarchy package structure (third party, unverified): <https://instagit.com/omacom/omarchy/how-is-omarchy-structured-as-arch-linux-packages.md>
- xrizer: <https://github.com/Supreeeme/xrizer>
- Monado with Steam, vronlinux: <https://vronlinux.org/docs/fossvr/monado/>
- Repo: `docs/*.md`, `analysis/research/vr-home-options.md`, `analysis/steamvr-games.md`, `analysis/steamvr-native-linux.md`, `packaging/arch/PKGBUILD`
