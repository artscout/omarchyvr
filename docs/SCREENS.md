# VR screens (Work / desk mode)

In Work mode (the WayVR desk), you can have up to **four VR screens**:
- the desktop in the centre;
- one screen to its left, one to its right and one above it.

Each extra screen is a real Hyprland desktop. You can open apps on it, move apps to it, and focus it.

## In the app
**Home > Desktop / WayVR > Screens**: choose 1, 2, 3 or 4 screens. The default is 4.
- The choice is saved in `~/.config/pimax/gui-state.toml` as `screens = N`.
- It reaches the Work start as `pimax desk start --screens N`. The flag is only written when N is not 4, because 4 is
  the launcher's default.
- The start records it in `start-argv.txt`, so restarts and pimaxd keep it.

Under the selector, a readiness line shows the state of the screens:
- before the session: "VR screens: 4 at the next Work start";
- during the session: the virtual outputs that exist, for example "VR screens: 4 (SUNSHINE in the centre; virtual
  outputs left PIMAX-LEFT, right PIMAX-RIGHT, up PIMAX-UP)";
- if creating them failed: the reason, shown in yellow. The session still starts, with one screen.

## How it works
1. **Displays before VR.** The Work start (`desk start`, not Play or game mode) creates the extra screens **before
   Monado and WayVR start**. Each one is a Hyprland *headless* output, made with runtime calls only (`~/.config/hypr` is
   never edited):
   ```
   hyprctl eval 'hl.monitor({ output = "PIMAX-LEFT", mode = "1920x1080@60", position = "-1920x0", scale = 1 })'
   hyprctl output create headless PIMAX-LEFT
   ```
   - Each output has the main desktop output's size.
   - The outputs sit left of, right of and above the main output, so Hyprland's own "focus left/right/up" crosses to
     them too.
   - `desk stop` removes them once Monado has exited.
   - Outputs are **never created or removed while a monado-service runs**, because that can disturb the leased DP-1.
     The code refuses in that case.
   - Between the watchdog's stop and its restart, the outputs are kept.
2. **Idempotent.**
   - Outputs that already exist with the right size are kept.
   - Stale `PIMAX-*` outputs, from a crashed session or with a different count or size, are removed first.
   - Missing outputs are created.
   - Any failure is written to `<session dir>/screens.txt` and shown in the app; the session goes on with the
     screens that exist.
3. **Layout in VR.** WayVR names each screen after its output.
   - `wayvrctl desk-screens` (WayVR, eye-gaze branch) places the screens: the main one in front of the head, the others
     edge to edge left, right and above it, all facing the head.
   - The same command also shows or hides each screen.
   - Presets call it with `--place --hide-others`. In that case WayVR app panels are not used.
4. **Screens on demand.** `pimax desk screens watch` is a small process of the session (pimaxd leaf component
   `screens`). It follows Hyprland's event socket.
   - A VR screen is **shown while its workspace has an app, or while it has the focus**. Otherwise it is hidden, but it
     keeps running.
   - Presets set a limit, kept in `$XDG_RUNTIME_DIR/pimax-screens.state`:

     | preset | screens shown |
     |---|---|
     | cinema, work-1 | the main screen only |
     | work-3 | main + left + right (shown even when empty) |
     | assistant, none | every screen, on demand |

   - The **focused** screen is always shown, even over the limit: an app that opens there, or "go to the left
     screen", never lands on a screen you cannot see.
   - Voice screen commands lift the limit.
   - If the watcher fails, the screens just stay as they are, and pimaxd restarts it.
5. **Modes never close apps** (user report 2026-10-15). A mode (cinema, work-3, work-1, assistant) only changes how
   many screens show and where they are. It reads Hyprland and tells WayVR which screens to show; it never closes,
   kills or moves an app. A screen it hides keeps its apps, and the toast names them: "Cinema mode. Still open on
   hidden screens: left (google-chrome), right (pimax). Say "go to the left screen"". The next mode with more
   screens (work-3, assistant) shows them again. `--hide-others` only hides other desktop captures (a mirrored
   output), never a WayVR app window or panel.
   - What happened in the report: "put everything in its own window" had put Chrome on the left screen and the
     Pimax app on the right one; "put everything in cinema mode" then hid both screens (cinema shows one), with no
     word about it, so the apps looked closed. Hyprland still had all three apps.

## The layout inside a screen (user decisions 2026-10-15)
One screen = one Hyprland workspace. The screens watcher applies these rules whenever apps open or move (not only on
the voice command):
- **One app alone fills its screen** (tiled and maximized).
- **Apps of the same kind share a screen, tiled to fill it**: 2 Chromes = half and half, 3 terminals = three tiles.
  With more (5 terminals), they keep tiling on the same screen.
- **Apps of different kinds never share a screen.** An app that opens on, or is sent to, a screen holding another
  kind goes to the screen that already has its kind, else to the next free screen (left, right, up, main). The kind
  that was there first stays.
- **Kinds** are by app class: browser (chrome, chromium, firefox, ...), terminal (foot, alacritty, kitty, ghostty),
  the Pimax app, and every other class is a kind of its own.
- Without a free screen, the app stays and shares its screen, tiled (never sent to a workspace no screen shows).
- Only apps that arrive on a screen (open or move there) are acted on, so an app you float or resize by hand stays
  that way until something changes on its screen. When an app leaves, the one left fills the screen again.
- Not touched: hidden workspaces, pinned windows, a real fullscreen (a video), and helper dialogs (file pickers,
  password prompts: classes `xdg-desktop-portal*`, `polkit*`, `pinentry`, `gcr-prompter`, `zenity`, ...).
- At the watcher's start nothing moves: it notes where the apps are.
- In cinema or work-1, an app sent to its own screen lifts the screen limit, so it does not vanish.
- "Put everything back together" turns the sorting off (`sort = off` in the state file): the kinds share the main
  screen on purpose. "Put everything in its own window" turns it on again. A mode switch keeps the setting.
- Hyprland 0.56 dispatchers used: `hl.dsp.focus({ window = "address:..." })` then
  `hl.dsp.window.move({ workspace = "N", follow = false })` (the focused app follows its move; the focus comes back
  after moving others), `hl.dsp.window.float({ action = "unset", window = ... })` (tile; the desktop config opens
  every window floating) and `hl.dsp.window.fullscreen({ mode = "maximized", action = "set"|"unset", window = ... })`.
  The `float` action names (`set`/`unset`, like `fullscreen`) were not checked against the live compositor (a Work
  session was running); a failing step is logged in `screens.log` and nothing else changes.

## Voice ("Max, ...")
The words follow the user's decisions of 2026-10-15 (docs/VOICE-DECISIONS.md):
- a **window** is always a whole VR screen (its workspace);
- "this" or an app's name means **one app**;
- a **tab** is inside an app.

| say | does |
|---|---|
| new window / open a new window | focuses the **next free screen**, in the order left, right, up (the first whose workspace has no app), and shows it. If every screen has apps, you get an empty workspace on this screen. |
| move this left / right / up / down, move the screen left | moves the **VR screen itself** in space: the screen you look at, else the focused one. It turns around your head by a quarter of its width or height and keeps facing you (`wayvrctl screen-nudge`). |
| bring it closer / push it away | the screen a quarter of its distance closer or farther (0.4 to 8 m) |
| zoom in / zoom out, make the screen bigger / smaller | the screen 25 % bigger or smaller, in place |
| send this to the left screen, move this app to the right screen, send this to the main screen | sends the **focused app** to the screen on that side of its own screen. From the left screen, "right" is the main one. From the up screen, "down" is the main one. |
| go to the left screen / switch to the right screen / focus the main (middle) screen | focuses that screen |
| put everything in its own window / organize / separate everything | sorts the apps of the shown screens **by kind**: terminals together (tiled splits), the browser, the Pimax app, and each other app class on its own. The kind of the app you are using stays where it is; the other kinds go to the left, right and up screens. When the screens run out, a kind stays where it is. With one screen, a kind goes to the next unused workspace. Hidden workspaces are not touched. Turns the watcher's sorting on. |
| put everything back together / merge everything | every app on the left, right and up screens joins the main screen, tiled. The screens then hide by themselves, because they are empty. Turns the watcher's sorting off until the next "put everything in its own window". |
| list windows | the VR screens with their apps: "4 screens \| Main: foot, firefox \| Left: empty \| Right: code \| Up: empty \| not shown: 3 (steam)" |
| close window | closes the whole screen's apps. It asks "say yes" only when there are 2 or more apps; with one app it closes at once ("close this" = one app, at once). |
| close the other windows / only this window / one screen | **shows one screen only** (the work-1 preset). It never closes workspaces or apps. |

The app has the same two buttons under **Screens**: **Put everything in its own window** and **Put everything back
together**. They are enabled while Work runs, and they run the same voice intent (`pimax-voice do --no-wake ...`).

### What you see with "put everything in its own window"
The Work session starts with all apps stacked on the big main screen: for example two terminals, Chrome, the Pimax
app and Obsidian. You are typing in a terminal. After "Max, put everything in its own window":
- the two terminals stay on the main screen, side by side as tiled splits;
- Chrome appears on a new screen to the left;
- the Pimax app appears on a screen to the right;
- Obsidian (another kind) appears on a screen above.

Each new screen shows up because it now has an app. The terminal keeps the focus. The toast says "Organized:
terminals on the main screen, browser on the left screen, Pimax app on the right screen, obsidian on the up screen.
Say "put everything back together" to undo".

"Max, put everything back together" moves Chrome, the Pimax app and Obsidian back onto the main screen, tiled with the
terminals. The left, right and up screens become empty and hide.

Typing ("type ...", "press enter") always goes to the **focused app**. It uses one persistent virtual keyboard per
voice process (see [VOICE.md](VOICE.md)).

## CLI (for agents and tests)
```
pimax desk start --screens 1|2|3|4         # default 4
pimax desk screens status                  # last start's report, the outputs, what VR shows
pimax desk screens sync [--dry-run]        # send the visibility to WayVR once
pimax desk screens create|remove [--screens N] [--dry-run]   # refused while a monado-service runs
pimax desk screens watch                   # the session's watcher (started by desk start)
wayvrctl desk-screens [--place] [--hide-others] [-w 2] [-d 2.5] [-c 0] [--gap 0.05] main=SUNSHINE left=PIMAX-LEFT right=PIMAX-RIGHT:hide up=PIMAX-UP:hide
```
Tests never reach the real compositor:
- `PIMAX_HYPRCTL` points the launcher at a fake `hyprctl`;
- the CLI tests put a fake one first on `PATH`.

## Limits
- A `hyprctl reload` drops the runtime monitor rules. The outputs stay, but Hyprland may place them elsewhere. Only
  "focus left/right" across monitors changes; what VR shows does not.
- Monitor rules added with `hyprctl eval` stay in Hyprland's memory until the next reload. They only match the
  `PIMAX-*` names.
- Code:
  - `rust/pimax-tools/src/vr/screens.rs` (outputs, state, watcher);
  - `rust/pimax-voice/src/windows.rs` (voice);
  - `rust/pimax-gui-core/src/screens.rs` and `rust/pimax-setup/src/pages/screens.rs` (app);
  - WayVR `desk_screens` in `wayvr/src/ipc/events.rs` and `grabmath::desk_neighbour`.
