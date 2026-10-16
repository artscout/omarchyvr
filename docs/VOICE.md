# Voice control ("Max, ...")

Everything in desk mode can be done by voice. The pipeline is:
- speech goes to the headset mic and `pimax-voice listen` turns it into text, locally;
- `pimax-voice act` turns the text into an intent (offline matcher, with an optional LLM fallback);
- the intent runs its action, and you see a short toast in the headset.

Nothing leaves the machine unless you set up a remote LLM yourself.

## How to talk
- **Wake word.** Start every command with **"Max"**, for example "Max, cinema mode" or "Max, volume up". It also
  catches the usual mis-hearings: "macs", "mars", "marks", and up to 2 leading filler words the speech model makes
  from "Pi"/"Hey"/"Ok" ("Hi Max", "B Max", "Five marks", "Time Max"). "Pimax" still works. It must come first
  (the "max" within the first 3 words), and the rest of the sentence must be a whole command. Normal talk, captions,
  and sentences that only contain a command or "max" ("set it to max", "I told Max to switch...") never trigger.
- **Command-word mishearings** are fixed by an alias table (`ALIASES` in `rust/pimax-voice/src/command.rs`):
  war/walk/wok/worth = work, tree/free = three, won = one, cinemas/sinema = cinema. "Time Max War Three" runs work-3.
- **Close is good enough.** Beyond the alias table, a fuzzy, sound-aware matcher scores every phrase ("liz windows"
  and "leaks list windows" both run list windows); a clear winner runs at once, a plausible one is asked about ("Did
  you mean "list windows"? Say yes"), and with the voice AI on, everything else goes to the model with the best
  candidates as hints (see [AI.md](AI.md) section 3). Noise after "Max" ("max ma max max", "can you hear me") is
  ignored without a toast.
- **Debug line.** With the voice AI on, a dim cyan line under the captions shows what the matcher and the model
  decided and how long it took. "Max, debug off" / "Max, debug on" ([AI.md](AI.md) section 12).
- **"Max" on its own.** You get a "Listening..." toast, and your next sentence (within 6 s) is taken as the command.
- **Filler words are ignored:** please, the, a, can you, could you, por favor, and so on. "Max, could you please
  switch to the cinema mode" works.
- **Languages.** English phrases plus some Spanish ones ("Max, modo cine"). Spoken numbers work: "work three",
  "remind me in ten minutes ...".
- **Feedback.** Every command shows a toast in the headset (WayVR notification) saying what was done, or "Didn't get
  ...". Info commands (battery, network, CPU) show their answer in the toast.
- **Confirmation** (`voice_confirm` in ai.toml, the **Confirm** selector on the app's AI page):
  - `none` (the default): every command runs at once, lock, close window and typing into a terminal included.
    Everything is logged (typed text in `~/.local/state/pimax/logs/typed.log`).
  - `dangerous`: lock, close window / app / tab, and typing or Enter into a terminal ask "...? Say yes" first.
  - `always`: every command asks.

  When it asks, say **"yes"** within 8 s, without "Max". Anything else cancels it.
  - Locking turns off the desktop, and the password cannot be typed in the headset. Take the headset off to unlock.
- **Undo.** "Max, undo" or "Max, go back" reverts the last preset or background change. It works several steps
  back.
- **Help.** "Max, help" or "Max, what can I say" shows a short list.

## Presets (desk mode layouts)
`pimax desk start` now uses **cinema by default**:
- use `--preset work-3|work-1|assistant` to start in another preset;
- use `--preset none` for no preset (the old default).

You can switch live by voice, or with `pimax desk preset NAME` (add `--dry-run` to only print the steps). A live
switch restarts nothing.

| preset | screens | passthrough |
|---|---|---|
| **cinema** (default) | one big curved screen (3.2 m wide, 3.5 m away) | dimmed (gain 0.35) |
| **work-3** (`work` is an alias) | three flat screens in an arc: the desktop in the middle, two app panels left and right | normal |
| **work-1** | one flat screen (2 m wide, 2.2 m away) | normal |
| **assistant** | all screens, slightly curved | normal; at start it also turns on the voice captions |

How the live switch works:
- It rewrites `conf.d/60-preset.yaml` in the WayVR home (used at the next start too).
- It sets the passthrough through its control socket.
- It runs `wayvrctl desk-layout N`, which shows the first N screens in an arc and hides the others (they are not
  closed).
- A mode never closes, kills or moves an app. With VR screens it shows or hides whole screens; a hidden screen keeps
  its apps and the toast names them ("Cinema mode. Still open on hidden screens: left (google-chrome). Say "go to
  the left screen""). See [SCREENS.md](SCREENS.md).
- The capture resolution (`screen_max_height`) changes only at the next start.

### VR screens (Screens = 2 to 4, the default)
The Work start creates up to three more Hyprland outputs before VR starts: `PIMAX-LEFT`, `PIMAX-RIGHT` and `PIMAX-UP`
(headless, the desktop's size, removed after the session). WayVR places them left of, right of and above the main
screen and shows each one only while it has an app or the focus. Presets set how many are shown: cinema and work-1
show the main screen, work-3 shows the main one plus left and right, and assistant shows every screen on demand. The
details are in [SCREENS.md](SCREENS.md).

### work-3 with one screen (Screens = 1)
With one VR screen, work-3 makes up its side screens from **WayVR app panels**:
- These are Wayland apps running inside WayVR's own compositor, started with `wayvrctl process-launch` and named
  `work-left` and `work-right`.
- The defaults are a `foot` terminal on each side. Change them in `~/.config/pimax/work-panels.conf`:
  ```
  # ~/.config/pimax/work-panels.conf
  left = foot
  right = nautilus --new-window
  ```
- Panels that are already running are reused. work-1 and cinema hide them (they keep running), and work-3 shows them
  again. To close one, close the app from inside it.

## Background (the VR backdrop)
"Max, next background" cycles through this list:

| name | what you see | how it is done |
|---|---|---|
| passthrough | the room, normal brightness | passthrough on, gain 1 |
| dimmed | the room, darker | passthrough gain 0.35 |
| black | black | passthrough off (it submits no layer) |
| sky | WayVR's built-in sky | `wayvrctl skybox default` drawn over the main app, then passthrough off |
| *your images* | equirectangular skybox images | `.dds` files in `~/src/desk-out/wayvr-home-am/config/wayvr/skybox/`, named by file name |

Other ways to set it:
- "Max, background sky", "Max, mountain lake background" or "Max, black background" pick one by name.
- "Max, passthrough on/off", "Max, dim", "Max, brighter" and "Max, darker" control the room view directly.

The **desktop wallpaper** is different: "Max, next wallpaper" or "Max, desktop background" runs Omarchy's
`omarchy-theme-bg-next` (row 37).

## Full phrase list
Generated from `rust/pimax-voice/voice-intents.toml`; `pimax-voice intents` prints the live list, including your
override. `@...` actions are built into `pimax-voice act`.

### VR presets

| say "Max ..." | does | intent (row) |
|---|---|---|
| cinema / cinema mode / movie mode / movie time / switch to cinema / switch to cinema mode / go to cinema mode / cinema preset / modo cine / cine | `@preset cinema` | vr.preset.cinema (vr) |
| work / work mode / work three / work 3 / work mode three / three screens / 3 screens / work with three screens / switch to work / switch to work mode / modo trabajo / tres pantallas | `@preset work-3` | vr.preset.work3 (vr) |
| work one / work 1 / work mode one / one screen / 1 screen / single screen / work with one screen / una pantalla | `@preset work-1` | vr.preset.work1 (vr) |
| assistant / assistant mode / switch to assistant / modo asistente | `@preset assistant` | vr.preset.assistant (vr) |

### Passthrough and background

| say "Max ..." | does | intent (row) |
|---|---|---|
| passthrough / passthrough on / turn on passthrough / show passthrough / show the room / show my room / i want to see the room / enciende passthrough / muestra la habitacion | `@backdrop passthrough` | vr.passthrough.on (vr) |
| passthrough off / turn off passthrough / hide passthrough / hide the room / black background / black / go black / apaga passthrough / fondo negro | `@backdrop black` | vr.passthrough.off (vr) |
| dim / dim passthrough / dim the room / dimmed / dimmed passthrough / darken the room / oscurece | `@backdrop dimmed` | vr.passthrough.dim (vr) |
| brighter / passthrough brighter / brighter room / make the room brighter / more light / mas brillo | `@passthrough brighter` | vr.passthrough.brighter (vr) |
| darker / passthrough darker / darker room / make the room darker / less light / menos brillo | `@passthrough darker` | vr.passthrough.darker (vr) |
| next background / change background / change the background / another background / next backdrop / next environment / siguiente fondo / cambia el fondo | `@backdrop next` | vr.backdrop.next (vr) |
| previous background / last background / previous backdrop / previous environment / fondo anterior | `@backdrop previous` | vr.backdrop.previous (vr) |
| background {name} / set background to {name} / change background to {name} / switch background to {name} / {name} background / environment {name} / fondo {name} | `@backdrop {name}` | vr.backdrop.set (vr) |

### Screens and view

| say "Max ..." | does | intent (row) |
|---|---|---|
| reset screens / reset the screens / screens back / bring the screens back / bring back the screens / where are my screens / resetea las pantallas | `{wayvrctl} reset-screens` | vr.screens.reset (vr) |
| recenter / recenter view / recenter the view / center the view / recentra / recentra la vista / centra la vista | `{wayvrctl} recenter` | vr.recenter (vr) |
| use controllers / use the controllers / prioritize controllers / prioritise controllers / turn on controllers / turn on the controllers / controllers first / controllers as pointer / controllers over hands / prioriza los mandos / usa los mandos | `{wayvrctl} controllers on` | vr.input.controllers (vr) |
| hands only / prioritize hands / prioritise hands / turn off controllers / turn off the controllers / hands first / use my hands / use hands / hands as pointer / hands over controllers / prioriza las manos / usa las manos | `{wayvrctl} controllers off` | vr.input.hands (vr) |
| show fps / fps / show performance / show frame rate / fps on / muestra fps | `{wayvrctl} panel-modify perf perf_box set-visible 1` | vr.fps.show (vr) |
| hide fps / fps off / hide performance / hide frame rate / oculta fps | `{wayvrctl} panel-modify perf perf_box set-visible 0` | vr.fps.hide (vr) |

### Voice

| say "Max ..." | does | intent (row) |
|---|---|---|
| captions on / show captions / turn on captions / subtitles on / show subtitles / subtitulos | `@captions on` | voice.captions.on (vr) |
| captions off / hide captions / turn off captions / subtitles off / hide subtitles / quita subtitulos | `@captions off` | voice.captions.off (vr) |
| debug on / show debug / debug mode on / turn on debug / show the debug line / debug mode | `@debug on` | voice.debug.on (vr) |
| debug off / hide debug / debug mode off / turn off debug / hide the debug line / no debug | `@debug off` | voice.debug.off (vr) |
| help / what can i say / what can you do / commands / list commands / ayuda / que puedo decir | `@help` | voice.help (vr) |
| undo / go back / back / undo that / revert / deshacer / vuelve / atras | `@undo` | voice.undo (vr) |
| cancel / never mind / nevermind / stop / forget it / cancela / olvidalo | `@cancel` | voice.cancel (vr) |

### Omarchy desktop

| say "Max ..." | does | intent (row) |
|---|---|---|
| fullscreen / make it fullscreen / go fullscreen / fullscreen this / pantalla completa | `hyprctl dispatch hl.dsp.window.fullscreen({ mode = "fulls...` | omarchy.window.fullscreen (omarchy#1) |
| full width / make it full width / maximize / maximize it / maximiza | `hyprctl dispatch hl.dsp.window.fullscreen({ mode = "maxim...` | omarchy.window.maximize (omarchy#2) |
| tiled fullscreen / tiled full screen toggle | `omarchy-hyprland-window-tiled-fullscreen-toggle` | omarchy.window.tiled_fullscreen (omarchy#3) |
| float this / float it / float window / tile it again / tile it / toggle floating / flota | `hyprctl dispatch hl.dsp.window.float({ action = "toggle" })` | omarchy.window.float (omarchy#4) |
| pop it out / pop it out and pin it / pop out / pin it | `omarchy-hyprland-window-pop` | omarchy.window.pop (omarchy#5) |
| back to the saved width / restore width / saved width | `omarchy-hyprland-window-width restore` | omarchy.window.width_restore (omarchy#6) |
| wider / make it wider / bit wider | `hyprctl dispatch hl.dsp.window.resize({ x = 100, y = 0, r...` | omarchy.window.wider (omarchy#7) |
| a lot wider / much wider / lot wider / make it a lot wider / make it much wider | `hyprctl dispatch hl.dsp.window.resize({ x = 300, y = 0, r...` | omarchy.window.much_wider (omarchy#7) |
| focus left / focus the window on the left / window on the left / left window | `hyprctl dispatch hl.dsp.focus({ direction = "l" })` | omarchy.focus.left (omarchy#8) |
| focus right / focus the window on the right / window on the right / right window | `hyprctl dispatch hl.dsp.focus({ direction = "r" })` | omarchy.focus.right (omarchy#8) |
| focus up / focus the window above / window above | `hyprctl dispatch hl.dsp.focus({ direction = "u" })` | omarchy.focus.up (omarchy#8) |
| focus down / focus the window below / window below | `hyprctl dispatch hl.dsp.focus({ direction = "d" })` | omarchy.focus.down (omarchy#8) |
| cycle windows / cycle through windows | `hyprctl dispatch hl.dsp.window.cycle_next()` ("next window" is the next workspace, below) | omarchy.window.next (omarchy#9) |
| flip the split / flip split / toggle split / rotate the split | `hyprctl dispatch hl.dsp.layout("togglesplit")` | omarchy.layout.flip_split (omarchy#10) |
| focus {app} / switch to {app} / go to {app} / show {app} / switch to {app} window / go to {app} window | `@win focus {app}` (fuzzy on class and title) | omarchy.focus.app (omarchy#11) |
| send this to workspace {n} / send it to workspace {n} / move this to workspace {n} / send to workspace {n} / send this to workspace {n} stay here / move it to workspace {n} / move window to workspace {n} / move this window to workspace {n} / mueve esto al workspace {n} / move this to window {n} / send this to window {n} / send it to window {n} | `hyprctl dispatch hl.dsp.window.move({ workspace = "{n}", ...` | omarchy.window.to_workspace (omarchy#16) |
| hide this in the scratchpad / hide this / to the scratchpad / send to scratchpad / scratchpad this | `hyprctl dispatch hl.dsp.window.move({ workspace = "specia...` | omarchy.scratchpad.hide (omarchy#17) |
| scratchpad / show the scratchpad / hide the scratchpad / toggle scratchpad / show or hide the scratchpad | `hyprctl dispatch hl.dsp.workspace.toggle_special("scratch...` | omarchy.scratchpad.toggle (omarchy#18) |
| open a terminal / open terminal / terminal / new terminal / abre una terminal / abre la terminal | `omarchy-launch-terminal` | omarchy.launch.terminal (omarchy#19) |
| open the browser / open browser / browser / open a browser / abre el navegador | `omarchy-launch-browser` | omarchy.launch.browser (omarchy#20) |
| open the file manager / open files / file manager / files / abre los archivos | `omarchy-launch-nautilus` | omarchy.launch.files (omarchy#21) |
| open obsidian / obsidian / switch to obsidian | `omarchy-launch-or-focus ^obsidian$ uwsm-app -- obsidian` | omarchy.launch_or_focus.obsidian (omarchy#22) |
| open spotify / spotify / switch to spotify / open music | `omarchy-launch-or-focus spotify uwsm-app -- spotify` | omarchy.launch_or_focus.spotify (omarchy#22) |
| open signal / signal / switch to signal | `omarchy-launch-or-focus ^signal$ uwsm-app -- signal-desktop` | omarchy.launch_or_focus.signal (omarchy#22) |
| volume up / louder / turn it up / sube el volumen / mas volumen | `omarchy-audio-output-volume raise` | omarchy.volume.up (omarchy#24) |
| volume down / quieter / turn it down / baja el volumen / menos volumen | `omarchy-audio-output-volume lower` | omarchy.volume.down (omarchy#25) |
| mute / unmute / toggle mute / mute sound / silencio | `omarchy-audio-output-volume mute-toggle` | omarchy.volume.mute_toggle (omarchy#26) |
| next song / next track / skip / skip song / siguiente cancion | `omarchy-shell media next` | omarchy.media.next (omarchy#27) |
| previous song / previous track / last song / cancion anterior | `omarchy-shell media previous` | omarchy.media.previous (omarchy#27) |
| pause / play / play pause / pause music / play music / resume music / pausa | `omarchy-shell media playPause` | omarchy.media.play_pause (omarchy#28) |
| how are cpu and memory / cpu and memory / system stats / stats / cpu / memory / how is the cpu | `omarchy-system-stats` | omarchy.info.stats (omarchy#31) |
| battery level / battery / how is the battery / bateria | `omarchy-battery-status` | omarchy.info.battery (omarchy#32) |
| network status / network / am i online / wifi / red | `omarchy-network-status` | omarchy.info.network (omarchy#33) |
| remind me in {minutes} minutes to {text} / remind me in {minutes} minutes {text} / remind me in {minutes} minute to {text} / reminder in {minutes} minutes {text} / recuerdame en {minutes} minutos {text} | `omarchy-reminder {minutes} {text}` | omarchy.reminder (omarchy#34) |
| take a screenshot / screenshot / capture the screen / captura de pantalla | `omarchy-capture-screenshot fullscreen save` | omarchy.screenshot (omarchy#35) |
| dismiss all notifications / dismiss notifications / clear notifications / borra las notificaciones | `omarchy-shell notifications dismissAll` | omarchy.notifications.dismiss (omarchy#36) |
| next wallpaper / change wallpaper / change the wallpaper / desktop background / next desktop background / siguiente fondo de pantalla | `omarchy-theme-bg-next` | omarchy.wallpaper.next (omarchy#37) |
| lock the screen / lock screen / lock / lock the computer / bloquea la pantalla | `omarchy-system-lock` (asks "yes") | omarchy.lock (omarchy#42) |


### Window control (user request 2026-10-15)
Workspaces, windows and tabs. "Max, fullscreen", "focus left/right/up/down" and "move this to workspace {n}" are in the
Omarchy table above. Details and the browser/terminal findings: `analysis/research/omarchy-window-control.md`.

The words (user decisions 2026-10-15, docs/VOICE-DECISIONS.md; the old Omarchy phrases "focus the window on the
left", "cycle windows", "float window" are now "app" phrases):
- **window** = a Hyprland workspace = one VR screen (a VR screen shows the workspace of its monitor). A Hyprland
  window is an **app**.
- **tab** = something inside an app: a browser tab, a terminal split (foot has no tabs), or a Hyprland group member.
- **the target** is what you are looking at (WayVR gaze: the last stable fixation, at most 2.5 s old), unless the
  command names one (a number, an app). Without gaze (eye tracking off, no fixation) it is the focused app/monitor.

| say "Max ..." | does | intent |
|---|---|---|
| go to workspace {n} / switch to workspace {n} / workspace {n} / show workspace {n} / window {n} / go to window {n} / switch to window {n} / show window {n} (1 to 10) | `hl.dsp.focus({ workspace = "N" })` | win.ws.goto |
| next workspace / workspace next / workspace forward / next window | `... workspace = "e+1"` | win.ws.next |
| previous workspace / workspace previous / workspace back / previous window | `... workspace = "e-1"` | win.ws.previous |
| new window / open a new window / nueva ventana (one VR screen) | an empty workspace on the looked-at screen (else the focused one): `hl.dsp.focus({ monitor = "NAME" })`, then `... workspace = "emptym"` | win.ws.new |
| switch to / go to / focus / show {app} (terminal, browser, files, steam, a title word, ...) | `@win focus {app}`: class, then title; saying it twice cycles windows of the same app | omarchy.focus.app |
| focus / focus this / focus that / select this | focuses the app you are looking at | win.focus.gaze |
| move this here / bring this here / merge this here / put this here / combine here | moves the focused app into the workspace you are looking at (it tiles in as a split) and keeps it focused | win.move.here |
| merge window {n} here / merge workspace {n} here / combine window {n} here | moves every app of workspace N into the workspace you are looking at | win.merge.here |
| next tab / tab next / tab forward | group tab, else browser Ctrl+PageDown, else the next terminal split on the workspace | win.tab.next |
| previous tab / tab previous / tab back | group tab, else browser Ctrl+PageUp, else the previous terminal split | win.tab.previous |
| tab {n} / go to tab {n} / switch to tab {n} (9 or more = last) | group tab N, else browser Ctrl+N (terminal splits have no numbers) | win.tab.goto |
| new tab / open a new tab | browser Ctrl+T; in a terminal a new terminal split (`omarchy-launch-terminal`); not in a group | win.tab.new |
| list windows / show windows / what windows are open | the VR screens with their apps ("Main: foot, firefox \| Left: empty \| ..."), then apps no screen shows | win.list |
| go to / switch to / find window where {text} (also "with", "about") | the AI switch (needs a provider) | win.ai.switch |
| new window / new screen (with VR screens) | the next free VR screen (left, right, up): the first whose workspace has no app; all busy = an empty workspace here | win.ws.new |
| move this left / right / up / down (the VR screen itself, in space), bring it closer / push it away, zoom in / zoom out | `wayvrctl screen-nudge` on the looked-at (else focused) screen ([SCREENS.md](SCREENS.md)) | win.screen.nudge / closer / farther / bigger / smaller |
| send this to the left / right / up / main screen, move this app to the left screen | sends the focused app to that VR screen | win.screen.move |
| put everything in its own window / organize / separate everything | sorts the apps by kind onto the VR screens (terminals, browser, Pimax app, the rest) | win.organize |
| put everything back together / merge everything | every app back on the main screen | win.together |
| go to / switch to / focus the left / right / up / main screen | focuses that VR screen | win.screen.goto |
| close the other windows / only this window / just this window | shows **one screen only** (the work-1 preset); never closes workspaces or apps | vr.preset.work1 |
| close window / close this window / close the window / cierra la ventana (asks "yes" only with 2+ apps on the screen) | closes the workspace: every app on the looked-at (else focused) workspace; **careful** ("Close window 3 (4 apps)?") | win.ws.close |
| close this / close it / close this app / quit this | closes one app: the one you are looking at, else the focused one; **careful** | win.close |
| close this tab / close tab | closes the browser tab, group tab or terminal split; **careful** | win.tab.close |

ASR forms handled: "work space"/"war space" = workspace, "tab to/for" = tab 2/4, "tabs/tap" = tab, "prev" = previous.
Terminals (foot, Alacritty, kitty, ghostty, wezterm) have no tabs of their own: a "tab" there is a terminal split on the
same workspace. Careful intents ask for a spoken "yes" only under the `voice_confirm` policy (`none` by default: they
run at once; `dangerous` asks for these; `always` asks for everything). With `none`, "close window" closes a whole
workspace without asking.

### Typing, keys and the browser (user request 2026-10-15)
Typing and keys go to the **focused app** only: the virtual keyboard has no target, the compositor delivers to the
focused surface. Since 2026-10-15 the voice process keeps **one** Wayland virtual keyboard open with one fixed keymap
(ASCII, Latin-1 with the Spanish letters, the named keys; `rust/pimax-voice/src/vkbd.rs`) instead of a new `wtype`
per command: every new virtual keyboard made Hyprland switch keymaps and terminals lost or garbled the first
character. After 1.5 s idle it taps Shift and waits 60 ms first, so Hyprland has switched to its keymap. If it cannot
connect, the command falls back to `wtype`. Into a terminal (foot,
kitty, Alacritty, ghostty, ...) Enter is never pressed without the policy's yes, copy and paste use Ctrl+Shift, and
select all / undo / redo are refused there. At most 200 characters per command.

| say "Max ..." | does | intent |
|---|---|---|
| type {text} / write {text} / type in {text} / escribe {text} | types the words as heard (case and punctuation kept), no Enter | type.text |
| press {key} / hit {key} / press the {key} key | enter, escape, tab, backspace, space, select all, copy, paste, undo, redo, page up/down, top, bottom, end, reload, back, forward | key.press |
| enter / press enter / send it / submit | Enter | key.enter |
| escape / select all / copy (that) / paste (it) / scroll down / scroll up | the key or shortcut | key.escape, key.select_all, key.copy, key.paste, key.scroll_down/up |
| dictate / start dictation / dictation mode / dictado | types everything you say next, without "Max", until "stop dictation" | type.dictate |
| previous page / browser back / reload / refresh | the browser's Alt+Left / F5 (only when a browser is focused) | browser.back, browser.reload |
| search for {q} / google {q} / look up {q} / busca {q} | a Google search in the browser (a new tab of the open browser, else a new browser) | browser.search |
| open {site} website / visit {site} / go to {site} website / open {site} in the browser | a known name (youtube, github, gmail, maps, ...) or any address ("example dot com"); an open tab with it is shown instead; not a site = a search | browser.open |
| go to the {q} tab / switch to the {q} tab / find the {q} tab / the tab with {q} | a **tab** (inside the browser): an app whose title has the words, else Ctrl+Tab through each browser (at most 25 tabs, back to the start when not found), else open {q} as a site | browser.tab.focus |

"Open the browser" focuses the open browser (any workspace) and starts one only when none runs. A request with
"and"/"then" can run up to 3 steps through the model: "Max, open youtube and search for cats".

### Recording (docs/RECORD.md)

| say "Max ..." | does | intent |
|---|---|---|
| start recording / start a recording / record / record video / record this / start the recording / empieza a grabar / graba / grabar | `pimax record start` with the saved settings (the toast shows its answer) | vr.record.start |
| stop recording / stop the recording / end recording / end the recording / finish recording / para de grabar / deja de grabar / termina la grabacion | `pimax record stop`: saves the file, the toast says where | vr.record.stop |
| am i recording / are we recording / recording status / is it recording / estoy grabando | `pimax record status` | vr.record.status |

### Not available by voice
The user decided on 2026-10-15 not to approve these rows of
`analysis/research/omarchy-voice-commands.html`:
- the careful rows 12-15, 23, 29, 30, 38 and 39;
- the never rows 40, 41 and 43-48: shut down, reboot, suspend, log out, close windows, workspace switching, mic mute,
  output switch, menus, themes, monitors, brightness, reload, update.

The loader drops any intent for these rows, their programs, or `hyprctl reload/dpms/keyword/...`, even from your
override file. Later, on the same day, the user asked for window control (table above): workspace switching, tabs,
and closing an app, a tab or a window (= workspace) with a spoken "yes". Those run through the built-in `@win` action,
which can only focus, switch to or create workspaces, move apps between workspaces, cycle, pick or open tabs, list, and
close apps (one, or all of one workspace); the loader refuses an `@win` close action without `confirm = true`. Shutdown, logout and Hyprland exit/kill stay impossible.

## Checking every command (`pimax-voice audit`)
The app's AI page has **Check all voice commands** (and **Open the last check**). It runs `pimax-voice audit` at
nice 19: every intent's first phrase, the same phrase as the speech-to-text might mangle it ("work" -> "war",
"list" -> "liz"), and ~200 natural phrasings (`rust/pimax-voice/tests/fixtures/audit.tsv`) go through the whole route
(exact, fuzzy, model) in a **dry run**: nothing is executed, typed or opened. It pauses 1 s after each model request
(`--pause-ms`) so the desktop stays responsive, and it refuses to start during a VR session (the live voice commands
need the model). Results: [AI.md](AI.md) section 13 and `analysis/voice-audit.html`.

`--live` (CLI only, for agents) also types for real into a scratch terminal the audit opens and closes itself
(`foot` running `cat`, closed by its PID): "type hello pimax", Enter, Backspace, checked in what `cat` received.
Keys are sent only while that window has the focus; browser, window and workspace commands stay in the dry run.

## Your own changes: `~/.config/pimax/voice-intents.toml`
This file is optional and not created for you. It uses the same format as the built-in catalogue (the comments at the
top of `rust/pimax-voice/voice-intents.toml` explain each key).
- An `[[intent]]` with an existing `id` sets `enabled`, adds `phrases`, and can replace `run`, `slots` and `say`.
- A new `id` adds a command.
- `[wake]`, `[paths]`, `[backdrop]` and `[llm]` replace the defaults.

```toml
# ~/.config/pimax/voice-intents.toml
[[intent]]
id = "omarchy.screenshot"
enabled = false                      # turn one off

[[intent]]
id = "vr.preset.cinema"
phrases = ["popcorn time"]           # more ways to say it

[[intent]]
id = "my.slack"
phrases = ["open slack", "slack"]
run = ["omarchy-launch-or-focus", "^slack$", "uwsm-app -- slack"]
say = "Slack"

[wake]
words = ["max", "pimax", "computer"]
followup_s = 6.0
confirm_s = 8.0
```

## LLM fallback (optional)
The LLM is asked only when a sentence starts with "Max" and the offline matcher found nothing.
- The model gets the list of enabled intent ids.
- It must answer `{"intent": "<id>", "slots": {...}}`.
- The answer is checked against the catalogue (the id must exist and be enabled, and every slot must have the right
  type and safe characters). Anything else is dropped.
- Intents that ask for "yes" still ask.

Configure it in the override file:
```toml
[llm]
backend = "auto"     # default: local ollama (127.0.0.1:11434) if it answers, else off
# backend = "ollama"; model = "qwen2.5:3b"
# backend = "openai"; url = "https://api.openai.com/v1/chat/completions"; model = "gpt-4o-mini"
# api_key_file = "~/.config/pimax/llm.key"   (or api_key_env = "OPENAI_API_KEY"); never put a key in the repo
# backend = "off"
timeout_s = 6.0
```
Remote calls go through `curl -K -` (the key is on stdin, not on the command line). Nothing is installed for you. For
local ollama: `ollama pull qwen2.5:3b`.

## AI agent (voice_ai)
With `voice_ai = "fallback"` or `"always"` in `~/.config/pimax/ai.toml` (the app's AI tab), a model with tools handles
what the matcher does not understand, and deictic requests ("Max, close this", "the one I'm looking at") use the gaze.
When the agent is on, it replaces the `[llm]` fallback and the AI switch below for those requests. The tools are the
catalogue's own intents, so `confirm` intents still need "yes" and `never` rows are refused. Local model:
`pimax-ai install` / `serve --cpu` (in VR). The local model gets a compact request (the intent list once, cached;
per command only the fuzzy hints, the words and a little context) and answers in about 1.3 s on the CPU.
`pimax-voice aibench` measures it. Everything else is in [AI.md](AI.md).

## AI switch (optional)
For requests the offline matcher does not know ("Max, go to the window where I was reading about Monado"), or when no
window matches the name you said, a model can pick the window.
- **Setup:** a provider in `~/.config/pimax/ai.toml` (the GUI's AI tab writes it). No file or no provider = off.
  `pimax-voice act` logs "AI switch: ..." at start. Interim format, read by `llm::ChatClient`:
  ```toml
  [switch]
  kind = "anthropic"                 # anthropic | openai | openai-compatible
  # model = "claude-haiku-5-5"       # default for anthropic; required for openai kinds
  # api_key_file = "~/.config/pimax/anthropic.key"   (or api_key_env; default ANTHROPIC_API_KEY / OPENAI_API_KEY)
  # url = "http://127.0.0.1:11434/v1"   # openai-compatible (ollama, llama-server); /chat/completions is appended
  # timeout_s = 8
  ```
- **What leaves the machine** (only after "Max", only when it is used): your request words and, per window
  (the 40 most recently used), the window address, class, title cut to 80 characters and workspace number. Titles can
  contain private things (page titles, file names, chat names), which is why the AI switch is off until you set a
  provider. No process ids, command lines, URLs or screenshots are sent.
- **What it can do:** answer `{"intent":"focus_window","slots":{"address":"0x..."}}` with an address from that list, or
  `{"intent":"none"}`. Anything else (another intent, extra keys or slots, an address not in the list) is rejected and
  nothing runs. The worst a hostile window title can do is focus another listed window.
- **Order:** catalogue phrases first; then the catalogue LLM fallback above; then the AI switch.

## Running it
- **In desk mode:** `pimax desk start --voice` starts three processes.
  - `pimax-voice listen` (mic to text) and `pimax-voice captions` (captions panel).
  - **`pimax-voice act`** (commands): log `voiceact.log` in the session dir, undo state in
    `$XDG_RUNTIME_DIR/pimax-voice-act.state`.
- **Text instead of speech:**
  - `pimax-voice do "pimax cinema mode" --dry-run` prints what would happen.
  - `pimax-voice do "pimax lock the screen" "yes" --dry-run` tests a confirmation.
  - `--no-wake` implies "Max".
- **Listen without acting:** `pimax-voice act --dry-run` follows a running listener and prints the actions instead of
  doing them.
- **List:** `pimax-voice intents [--all]`.
- **Passthrough by hand:** `pimax-passthrough ctl on|off|dim|normal|brighter|darker|gain 0.5|status`.
  - The control socket is `$XDG_RUNTIME_DIR/pimax-passthrough.sock`.
- **WayVR by hand:**
  - `wayvrctl desk-layout 3 --curvature 0 --width 1.6 --distance 2`;
  - `wayvrctl skybox off|default|skybox/NAME.dds`;
  - `wayvrctl recenter`.

### What needs a rebuild or restart (do it with desk mode stopped)
```sh
pimax desk stop
cd ~/pimax-re/rust && cargo build --release -p pimax-voice -p pimax-passthrough -p pimax-tools
install -m755 target/release/pimax ~/.local/bin/pimax      # the launcher (desk preset, cinema default, voiceact)
cp ~/src/target-desk-test/release/wayvr ~/src/target-desk-test/release/wayvrctl ~/src/target-desk/release/
pimax desk start --voice
```
The launcher runs `pimax-voice` and `pimax-passthrough` from `~/pimax-re/rust/target/release`, so building them is
enough.
Older parts keep working:
- An older WayVR (before `aac483a`) has no `desk-layout` or `skybox`. Presets then only change the passthrough and
  `60-preset.yaml`, and backgrounds other than passthrough/dimmed/black report an error toast.
- An older passthrough has no control socket.

Live test steps: [analysis/voice-live-checklist.md](../analysis/voice-live-checklist.md).

## GUI integration (for pimax-setup / pimax-gui-core; not done here)
A **Voice** settings row could have:
- **Voice commands** on/off: start desk with `--voice`, or not.
- **Wake word**: edit `[wake] words` in `~/.config/pimax/voice-intents.toml`. Show the active list from
  `pimax-voice intents` (the "wake words:" line).
- **LLM backend**: off / auto (local ollama) / ollama model / OpenAI-compatible (url, model, key file). Write the
  `[llm]` table. Never store the key in the table itself; point `api_key_file` at a 0600 file.
- **Intent list with toggles**:
  - Fill it from `pimax-voice intents --all`, one line per intent: `id [safety, source, confirm] phrase | phrase`,
    with `(off)` in front when disabled.
  - A toggle writes `[[intent]] id = "..." enabled = true|false` to the override file.
  - Do not offer toggles for rows that are not approved: they are not in the list at all.
- **Preset picker**: cinema (default) / work-3 / work-1 / assistant / none.
  - The GUI today omits `--preset` for "none". That now means **cinema**, so pass `--preset none` explicitly for no
    preset.
  - While desk mode runs, "Apply now" runs `pimax desk preset NAME`.
- **Work panels**: two text fields for `left =` / `right =` in `~/.config/pimax/work-panels.conf`.
