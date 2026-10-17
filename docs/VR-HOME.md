# The VR home

Goal (user, 2026-10-17): "We have the blue WayVR setup that is really clean and not too much: can we have that as the
default. From there we can open SteamVR, do room setup (quick and full), go to WayVR Work, or start SteamVR, or directly
start xrizer VR games. Also allow users to start room setup with the Pi button."

Status: **built and tested with fakes; not yet tried in the headset.** Needs the WayVR build with fork commit
`26c40a35` (right menu button + `menu_open_at_start`); an older WayVR still shows the panel on the left menu button,
just not at start.

## What the user sees

Press **Play** in the app (or let start at login do it). The headset shows a small panel in WayVR's own look (navy card,
blue buttons, red Exit), 0.6 m in front of you, over the passthrough (or WayVR's sky):

| Entry | What it does |
|---|---|
| **Work** | the desk (screens, voice, AI); the home closes first |
| **Games with SteamVR** | SteamVR with the Pimax driver (its own home and dashboard); the home closes first |
| **one button per installed Steam VR game** (at most 6), labelled `(xrizer)` or `(SteamVR)` | xrizer games start right there on Monado; a game on the SteamVR list (`game-runtime.json`, e.g. Half-Life: Alyx) restarts into SteamVR for it |
| **Room setup: Quick (30 s) / Full (3 min)** | the guided room setup, in the headset, without leaving VR |
| **Recenter, Fix floor, Close** | a few settings |
| **Exit VR** | stops everything, like Stop in the app; press it twice |

A status line under the title says what is happening ("Starting Work: the VR home closes first ...") or what failed
last ("SteamVR did not start: Steam must restart for VR. Back to the VR home.").

In **Work** the controllers' menu button opens the Work menu, which is the same panel: **VR home** in place of Work,
the games, Room setup, Close menu, Recenter, WayVR settings and **Exit Work**.

## The menu (Pi) button

Evidence:

- The Pimax controller report has one menu button per hand: report byte 48 bit 0 (`rust/pimax-proto/src/controller.rs`
  `decode_controller`, `BTN_MENU = 0x10`; `analysis/controllers-linux.md`: confirmed on the left in the Windows
  captures, the right one was never pressed in a capture). No other system / home button exists in the report.
- Monado's Pimax driver (`forks/monado/src/xrt/drivers/pimax/pimax_ctrl.c`, input `I_MENU`) presents it as
  `XRT_INPUT_TOUCH_MENU_CLICK` on the left and `XRT_INPUT_TOUCH_SYSTEM_CLICK` on the right, i.e. the OpenXR paths
  `/user/hand/left/input/menu/click` and `/user/hand/right/input/system/click` (Monado's `bindings.json` exposes the
  right system click to apps).
- Under SteamVR, driver_monado maps the menu button of either hand to `/input/system/click` (docs/STEAMVR.md section 5;
  user test 2026-10-15: "all buttons good").

Mapping (no new HID bytes, only bindings): WayVR's `menu` action is bound to **both** paths on the Touch profile
(`forks/wayvr/wlx-common/assets/openxr_actions.json5`; tests: both bound, nothing else on either button, paths exist in
the controller schema). The action toggles the panel named by `menu_panel`:

| Session | Panel | Shown at start |
|---|---|---|
| VR home (`vr start --home`) | `vr_home` on the home overlay (minimal WayVR) | yes (`menu_open_at_start: true`) |
| Monado game session, "Menu button opens the VR home in games" on (default) | `vr_home` on the same overlay | no |
| Work | `work_menu` (the home's entries + Work's own) | no |
| SteamVR | none of ours: the button opens SteamVR's dashboard | - |

In a game, the game also receives the left menu button (Monado sends input to every focused session); many games open
their pause menu with it. The right button is the system button most games do not bind.

## Flow

```
                      app: Play  (Start in VR home on)            app: Work / Room tiles: unchanged
                              |
                              v
   +-------------------------------------------------------------+
   |  VR HOME  = vr start ... --home --runtime monado             |
   |  Monado + 6DoF + controllers + room anchor + passthrough     |
   |  + minimal WayVR with the vr_home panel (shown at start)     |
   +-------------------------------------------------------------+
     |  Work       |  Games with     |  game (xrizer)  |  game (SteamVR) | Room Quick/Full | Exit VR
     |             |  SteamVR        |                 |                 |                 |
     v             v                 v                 v                 v                 v
  vr stop       vr stop          stop the home's    vr stop          omarchyvr-roommap   vr stop
  wait gone     wait gone        passthrough;       wait gone        setup fast|full     (2 presses)
  desk start    vr start         steam://rungameid  vr start         (in this session)
  (app's Work   --runtime        (same Monado,      --runtime
   switches)    steamvr          overlay stays:     steamvr --game ID
                                 menu button =      steam://rungameid
                                 the panel)
     |                                 |
     v                                 v
   WORK  --menu button--> work_menu: VR home | SteamVR | games | Room | Exit Work
     |  VR home: desk stop (screens, voice, AI) -> wait -> vr start --home
```

Every switch is **one full stop, one wait until Monado / SteamVR are gone, one start**, the same commands as the app's
Stop and Play/Work buttons. One at a time: a flock on `<session dir>/home-transition.lock`; a press while one runs gets
"Still busy: ..." in the headset. A start that fails after the stop (the headset is dark) starts the home again, with
the reason on its status line.

Room setup needs 6DoF and the room anchor (Full also optical controller tracking). The home has them when 6DoF is on;
Work too. Otherwise (SteamVR, a game session, a home without controller tracking for Full) the transition goes to a
home with them first.

## Switches (app: Home > VR home; CLI: `omarchyvr home set ...`)

`~/.config/omarchyvr/home.toml`, shared by the app and the CLI:

| Key | Default | |
|---|---|---|
| `start_in_home` | true | Play lands in the home; off = Play starts the game session directly, as before |
| `pi_menu` | true | the home overlay also runs (hidden) in every Monado game session |
| `background` | passthrough | or `sky` (WayVR's own) |

`~/.config/omarchyvr/home-launch.json`: the app writes its own Work and Play launcher arguments at every start, so a
transition from the headset starts exactly what the app's buttons would (never `--force`). Without it: 90 Hz, 6DoF,
optical controllers (and the anchor for Work).

Play for a chosen game (the app's "next game", or the switch to SteamVR offered for a listed game) still starts that
game's session directly. Light game mode does not apply to the home (it keeps the anchor and room search for the room
setup; no wrist readout); a game started from the home runs in that session.

## Files and engine

| | |
|---|---|
| `rust/omarchyvr-tools/src/vr/home.rs` | settings, the games list, the panel layout (`panel_xml`), `classify`, `plan`, `execute`, the lock, the CLI |
| `rust/omarchyvr-tools/src/vr/perf.rs` | the minimal WayVR of a game session: the home panel and/or the wrist readout (`overlay_start`) |
| `rust/omarchyvr-tools/src/vr/mod.rs` | `vr start --home`; `vr stop` also stops the home's passthrough |
| `rust/omarchyvr-tools/src/vr/desk.rs` | the Work menu is `home::panel_xml(Ctx::Work, ...)` |
| `rust/omarchyvr-gui-core/src/vrhome.rs` | Play -> home start job, the switches, `home-launch.json`, the preview |
| `rust/omarchyvr-app/src/pages/home.rs` | the VR home card (switches + the panel's preview), "VR home running", the last transition |
| `forks/wayvr` `26c40a35` | right system click = `menu`; `menu_open_at_start` |
| `<session dir>/home-transition.txt` | the last transition: state, target, text, when |
| `<session dir>/home.log`, `room-setup.log` | the transitions' and the room setup's output |

```
omarchyvr home status | games | plan TARGET | go TARGET | run TARGET | panel [--work] | set KEY VALUE
TARGET: home | work | steamvr | game APPID | room-quick | room-full | exit
```

Games: Steam's `config/steamapps.vrmanifest` (the VR apps Steam knows) plus the entries of `game-runtime.json`, only
installed ones (an `appmanifest_<id>.acf` in a library folder of `libraryfolders.vdf`), each with its runtime
(`game-runtime.json`, else "Games run with").

## Tests

- omarchyvr-tools `vr::home`: settings and launch-args round trips; vrmanifest / library folders / games with their
  runtime; the panel has every entry with its command (home and Work); `classify`; every switch is one full stop then
  one start, said in the headset first; already-there is said, not restarted; games use their own runtime; room setup
  in place or via a home with what it needs; Exit; target words; the executor with a fake (stop, wait, start; a failed
  start falls back to the home once; no fallback before a stop); one transition at a time and Exit's two presses;
  status file.
- Golden dry runs: `vr-start-home.txt` (the panel shown at start, the passthrough, no readout, the anchor), and the
  hidden panel line in every Monado game start.
- omarchyvr-gui-core `vrhome`, `play`: Play lands in the home by default with Play's switches and Monado; off = the old
  Play; the home never asks about a tainted Steam; the preview; a recent transition on Home.
- WayVR: binding paths, `menu_open_at_start` parsing, `menu_shown_at_spawn`.
- Screenshots: `omarchyvr-app --screenshots DIR`: `home-vr-home*.png`.

## Open decisions

- Back from **SteamVR**: our button is SteamVR's there, so there is no VR home inside SteamVR. Leave with the app's
  Stop / Play, or SteamVR's own exit (then Play again). A WayVR overlay under SteamVR (OpenVR backend) could carry the
  panel later.
- A **game started from the home** runs next to the overlay (a few MB of video memory for a hidden panel) and in a
  session without Light game mode (anchor and room search on). Alternative: a full restart into a light game session
  for every game (slower, black for ~20 s).
- The **left menu button** reaches the game too (pause menus). Bind the home to the right button only in games?
- **Work's first run** questions (docs/WORK-FIRST-RUN.md) are asked in the app; Work from the headset before they were
  answered starts with the defaults.
- Room setup from a running **xrizer game** in the home runs over the game.
- Not tried in the headset yet: the panel's look and size (0.45 m wide), the right button on the real controller, the
  status toasts in the headless WayVR.
