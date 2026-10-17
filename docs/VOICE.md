# Voice control ("Max, ...")

Everything in desk mode can be done by voice. The pipeline is:
- speech goes to the headset mic and `omarchyvr-voice listen` turns it into text, locally;
- `omarchyvr-voice act` turns the text into an intent (offline matcher, with an optional LLM fallback);
- the intent runs its action, and you see a short toast in the headset.

Nothing leaves the machine unless you set up a remote LLM yourself.

## How to talk
- **Wake word.** Start every command with **"Max"**, for example "Max, cinema mode" or "Max, volume up". It also
  catches the usual mis-hearings: "macs", "mars", "marks", and up to 2 leading filler words the speech model makes
  from "Pi"/"Hey"/"Ok" ("Hi Max", "B Max", "Five marks", "Time Max"). "Pimax" still works. It must come first
  (the "max" within the first 3 words), and the rest of the sentence must be a whole command. Normal talk, captions,
  and sentences that only contain a command or "max" ("set it to max", "I told Max to switch...") never trigger.
- **Command-word mishearings** are fixed by an alias table (`ALIASES` in `rust/omarchyvr-voice/src/command.rs`):
  war/walk/wok/worth = work, tree/free = three, won = one, cinemas/sinema = cinema. "Time Max War Three" runs work-3.
- **Close is good enough.** Beyond the alias table, a fuzzy, sound-aware matcher scores every phrase ("liz windows"
  and "leaks list windows" both run list windows); a clear winner runs at once, a plausible one is asked about ("Did
  you mean "list windows"? Say yes"), and with the voice AI on, everything else goes to the model with the best
  candidates as hints (see [AI.md](AI.md) section 3). Noise after "Max" ("max ma max max", "can you hear me") is
  ignored without a toast.
- **Debug line.** With the voice AI on, a dim cyan line under the captions shows what the matcher and the model
  decided and how long it took. "Max, debug off" / "Max, debug on" ([AI.md](AI.md) section 12).
- **Claude through the CLI (the default AI).** With no other choice, "Max, ..." anything the matcher does not
  understand goes to your installed Claude Code (`claude -p`, your own login, no API key), a fresh process per
  command, one kept ready; see [AI.md](AI.md) section 20.
- **Where the AI runs.** The Work desk's watch has an "AI: ..." button: This PC (one model) / This PC, 2 stages (the
  router Lara sorts the command, the model on the graphics card answers) / Online. With two stages, two more buttons
  switch Lara and the model. Voice control follows within 2 s, no restart ([AI.md](AI.md) section 16).
- **Work only** (2026-10-17). Voice runs in Work, never in Play: Play stops any voice process a crashed Work left, and
  omarchyvrd never starts one in a game session. The first Work asks how speech becomes text (the processor, the
  graphics card next to the local model, or off) and who answers ([WORK-FIRST-RUN.md](WORK-FIRST-RUN.md)); "off"
  starts Work without voice. Exit Work (the Work menu on the left menu button, or the watch) stops voice with
  everything else ([WAYVR-CONTROLS.md](WAYVR-CONTROLS.md)).
- **Lara in front of an online model** (2026-10-17, `voice_lara = true`): with Claude through your key or another
  compatible API, Lara sorts each command on this PC (processor) and the online model gets only that category's
  tools; without Lara running, the matcher's guess is used and the online model still answers. The mode reads
  "Online, Lara sorts" ([AI.md](AI.md) section 21).
- **"Max" on its own.** You get a "Listening..." toast, and your next sentence (within 6 s) is taken as the command.
- **Pauses** (2026-10-16). A sentence that stops mid-way ("Max, can you move uh one of the", "can you write uh") is not
  run: the listener waits 1.1 s instead of 0.5 s for more words after "the", "of", "to", "uh", "can you" and similar
  endings (`segment.rs`: a decode 64 ms into each pause tells a finished command from a cut-off one), and when the
  sentence still ends there, `act` holds it and joins it with your next sentence (within 6 s, without "Max"):
  "Max, can you move all the" + "terminals to one window" runs as one command. Nothing comes: it is dropped.
  Hesitations ("uh", "um") are never typed.
- **"Max" later in a sentence** (2026-10-16). When the speech-to-text marks a pause next to "Max" ("Windows, can we ha
  Max, can we have two more windows?", "Okay. Max, cinema"), what follows "Max" runs. Without a pause mark ("I told
  Max to switch") nothing happens.
- **Filler words are ignored:** please, the, a, can you, could you, por favor, and so on. "Max, could you please
  switch to the cinema mode" works.
- **Languages.** English phrases plus some Spanish ones ("Max, modo cine"). Spoken numbers work: "work three",
  "remind me in ten minutes ...".
- **Feedback.** Every command shows a toast in the headset (WayVR notification) saying what was done, or "Didn't get
  ...". Info commands (battery, network, CPU) show their answer in the toast.
- **Confirmation** (`voice_confirm` in ai.toml, the **Confirm** selector on the app's AI page):
  - `none` (the default): every command runs at once, lock, close window and typing into a terminal included.
    Everything is logged (typed text in `~/.local/state/omarchyvr/logs/typed.log`).
  - `dangerous`: lock, close window / app / tab, and typing or Enter into a terminal ask "...? Say yes" first.
  - `always`: every command asks.

  When it asks, say **"yes"** within 8 s, without "Max". Anything else cancels it.
  - Locking turns off the desktop, and the password cannot be typed in the headset. Take the headset off to unlock.
- **Undo.** "Max, undo" or "Max, go back" reverts the last preset or background change. It works several steps
  back.
- **Help.** "Max, help" or "Max, what can I say" shows a short list.

## Presets (desk mode layouts)
`omarchyvr desk start` now uses **cinema by default**:
- use `--preset work-3|work-1|assistant` to start in another preset;
- use `--preset none` for no preset (the old default).

You can switch live by voice, or with `omarchyvr desk preset NAME` (add `--dry-run` to only print the steps). A live
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
- The defaults are a `foot` terminal on each side. Change them in `~/.config/omarchyvr/work-panels.conf`:
  ```
  # ~/.config/omarchyvr/work-panels.conf
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
| ocean:medium | the animated ocean (WayVR draws it live; low / medium / high) | `wayvrctl skybox ocean:<quality>`, then passthrough off. "Max, animated ocean", "moving ocean", "live sea", "ocean high"; plain "ocean" / "the sea" / "waves" pick it too ("beach" stays the photo) |
| *your images* | equirectangular skybox images | `.dds` files in `~/src/desk-out/wayvr-home-am/config/wayvr/skybox/`, named by file name |

Other ways to set it:
- "Max, background sky", "Max, mountain lake background" or "Max, black background" pick one by name.
- "Max, passthrough on/off", "Max, dim", "Max, brighter" and "Max, darker" control the room view directly.

The full library (66 free 360 pictures in 13 themes, the Backgrounds page of the app, resolutions, licences) is in
[BACKGROUNDS.md](BACKGROUNDS.md): "Max, background off" brings the room back, "Max, next background" steps through them.

The **desktop wallpaper** is different: "Max, next wallpaper" or "Max, desktop background" runs Omarchy's
`omarchy-theme-bg-next` (row 37).

## Full phrase list
Generated from `rust/omarchyvr-voice/voice-intents.toml`; `omarchyvr-voice intents` prints the live list, including your
override. `@...` actions are built into `omarchyvr-voice act`.

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
  "This app", "this" and "it" (close this app, send this to the left screen, put this on screen 3) mean the app you are looking
  at; when there is no gaze they mean the focused app.

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
| four screens / give me 4 screens / one screen (1 to 6) | shows that many VR screens: main, left, right, up, up-left, up-right (`omarchyvr desk screens show N`) | vr.screens.count |
| two more screens / add 2 screens / one more screen / another screen | N more than are shown now, up to 6 (`omarchyvr desk screens more N`) | vr.screens.more, vr.screens.one_more |
| a screen to the left / add a screen up there / a screen top right | shows that screen (`omarchyvr desk screens add SIDE`) | vr.screens.add |
| open / focus / put YouTube in a new window, put Spotify on its own screen | that app on a free VR screen of its own, focused: a browser tab moves alone (its address opens in a new browser window there, the page reloads), another app moves whole, not open = it opens there | win.screen.open |
| move all the terminals to one window / put all the browsers on one screen | every app of that kind joins the one in use; the other apps stay | win.screen.gather |
| go to / focus (in) the main window | the main (middle) VR screen | win.screen.goto |
| new window / new screen (with VR screens) | the next free VR screen (left, right, up): the first whose workspace has no app; all busy = an empty workspace here | win.ws.new |
| move this left / right / up / down (the VR screen itself, in space), bring it closer / push it away, zoom in / zoom out | `wayvrctl screen-nudge` on the looked-at (else focused) screen ([SCREENS.md](SCREENS.md)) | win.screen.nudge / closer / farther / bigger / smaller |
| send this to the left / right / up / main screen, move this app to the left screen | sends the app you are looking at (else the focused one) to that VR screen | win.screen.move |
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
(ASCII, Latin-1 with the Spanish letters, the named keys; `rust/omarchyvr-voice/src/vkbd.rs`) instead of a new `wtype`
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
"and"/"then"/"also" (or two commands back to back: "close this go to the main screen") can run up to 4 steps through
the model, in order: "Max, open youtube and search for cats", "Max, two more screens and put Chrome there". One toast
then lists every step's result ("2 of 2 steps: 1. Showing 6 screens | 2. google-chrome on the up-left screen").

### Apps to named screens, videos, agent mode (user requests 2026-10-16)

| say "Max ..." | does | intent |
|---|---|---|
| put all the terminals on the left screen / move Chrome to the right screen / YouTube on the up-right screen / put the code editor in the middle | every app of that kind or name (terminals = foot, kitty, Alacritty, ghostty...; browser = Chrome, Firefox...; a browser tab by its title; "code editor" = Code, Zed) goes to that VR screen, which shows; the other apps stay | win.screen.put |
| move Chrome to screen 3 / put all my terminals on screen two / the newest terminal to screen 5 | the same, to screen N (docs/SCREENS.md: 1 main, 2 left, 3 right, 4 up, 5 up-left, 6 up-right, the number on each screen); "one", "newest", "last" = only the one used last | win.screen.put_n |
| move the terminal from here to screen 4 / move Chrome from screen 2 to screen 5 | only the apps of that kind on the first screen ("here" = the screen looked at, else the focused one) | win.screen.put_from |
| move everything from screen 2 to screen 5 / from screen 3 to screen 1 | every app of one screen to another | win.screen.move_all |
| send this to screen 6 | the focused app to screen N | win.screen.move_n |
| go to screen 4 / what's on screen 3 | focus that screen / list its apps | win.screen.goto_n, win.screen.list_n |
| swap screens 1 and 2 | the apps of the two screens change places | win.screen.swap |
| make screen 2 bigger / bring screen one closer / move screen 3 left | that screen in space: bigger, smaller, closer, farther, left, right, up, down | win.screen.adjust |
| open YouTube on screen 3 / open https://... on screen 2 / open ~/Videos/x.mkv on screen 1 | that screen is focused, then the site (new browser window), address, file (its default app) or terminal / browser / files opens there | win.screen.open_n |
| save this layout (as coding) / restore the (coding) layout | which app is on which screen, saved in `~/.local/state/omarchyvr/voice-layouts.json`; restore moves the open apps back (by class, the closest title first), nothing opens or closes | win.layout.save(_as), win.layout.restore(_as) |
| show screen numbers / hide screen numbers / always show screen numbers | the number badge on each VR screen (`wayvrctl screen-numbers show|hide|always`). The numbers also flash by themselves for ~5 s when "Max" is heard alone or a command talks about screens, and after a move / send / go to / swap / open on screen N that screen's badge lights amber (`screen-numbers flash N`). An older WayVR without the command: nothing happens, nothing fails | vr.screens.numbers(_hide, _always) |
| set the volume to 40 / volume 70 percent | the volume to N percent (`wpctl -l 1.0`: never above 100) | omarchy.volume.set |
| play Dune / I want to watch Dune | the video whose file name has those words, in ~/Downloads, ~/Videos and the XDG videos folder only (3 folders deep, never the rest of home): the biggest match, then the newest ("Playing Dune.2021.mkv (the biggest of 2)"); fullscreen in mpv on the main screen (cinema mode shows that one). Not found: "No video called ..." and nothing else runs. Nothing is downloaded | media.play |
| huge cinema / imax / big cinema / cinema mode huge / theater mode / make it a huge screen | `@preset cinema-huge`: one giant curved screen, room black, captions off (docs/CINEMA.md) | vr.preset.cinema_huge |
| exit cinema mode / leave cinema mode / exit huge cinema / exit imax / normal screens | the mode from before cinema comes back, with its passthrough ("cinema mode" while it is on already changes nothing) | vr.preset.exit_cinema |
| pause / play | mpv answers the media keys (mpv-mpris) | omarchy.media.play_pause |
| cancel / cancel that / stop | stops a task of several steps (below); else a question waiting for "yes" | voice.cancel |

Every move to a screen is checked (2026-10-17, docs/SCREENS.md "What voice knows"): the screens and windows are read
fresh, only the windows not on the target move (by address), the state is read again, a window still not there gets
one more try, and the answer says what the last read shows ("Moved 1 terminal ...; 1 already there", "Could not move
foot 'build' to screen 3 (right), it is still on screen 1 (main)"). A screen the session does not have is never used
("There is no screen 5 (up-left) now ... restart Work"). A stutter is fine: "open the browser in in 4" = the browser on
screen 4.

### Screens by voice: hide, resize, stop (user rules 2026-10-16, live session 15:49)

| say "Max ..." | does | intent |
|---|---|---|
| close screen 2 / close down screen number two / remove screen number four and three / shut down screens 4 and 3 / hide screens 2, 3 and 5 / close window 2 / close this screen | those VR screens HIDE (`omarchyvr desk screens hide 4,3`); their apps stay open and come back with the screen ("add a screen on the right", "4 screens"). Never closes an app | vr.screens.hide |
| close everything / close the screens / close all the screens / close the other two screens | every screen but the main one hides; no app closes | vr.screens.hide_others |
| make all these screens half the size / make all the screens double the size / all screens 75 percent / make screen 2 half the size | every VR screen (or the one named) scaled to that size (`wayvrctl screen-nudge --step`: half = 1/2, double = x2, a percent) | win.screen.scale |
| move all the terminals to screen number three / put chrome on screen five | the apps of that kind or name to screen N (the screen number always reaches the action) | win.screen.put_n |
| move this screen number one to the right | screen 1 itself moves right in space | win.screen.adjust |
| move it in front of me (the screen) / bring the screen in front of me | the screens back in front of your head | vr.screens.reset |
| stop the VR / shut down work VR / exit VR / close work / stop the session | stops Work / VR exactly like the app's Stop button (`omarchyvr desk stop --grace 30`). **The one command that always asks "yes" first**, whatever `voice_confirm` says | vr.session.stop |

These are decided by fixed rules (`rules.rs`) before the model is asked: a request that names a VR screen by number,
stops the session or resizes screens never depends on the model. "Close window" (a workspace with its apps,
`win.ws.close`) never runs for a request that names a screen or a number: that hides the screen. And "close window"
is pinned to the workspace it meant when it was heard: the question names it ("Close window 3 (2 apps)?") and the
"yes" closes exactly that one, never the one looked at a few seconds later (regression test
`close_window_closes_only_the_window_named_and_never_for_a_screen`).

"Max" in the middle of a sentence: "Can you Max can you shut down ...", "uh Max can you ...", "All of them. Can you
make Max, can you make ..." act on the words after the last "Max". When those are only a fragment ("Can you move it in
front of me, Max, the screen."), the words before "Max" stay. "I told Max to switch" and "set it to max" still never
wake.

The model sees the VR screens by number with their apps when a request is about screens:
`screens: 1 main [w1 foot, w2 foot] ; 2 left [] ; 3 right hidden [w3 pimax]` (the `wN` of the apps line; `hidden` =
hidden by voice).

**Agent mode** (`voice_agent = true` in ai.toml, the default; `agent::goal`). A request that is a goal of several
steps, with a goal word ("set up", "let's", "I want to watch", "get ready", "prepare") and more than one thing in it, or
a long request with "and"/"then"/two commas, runs one step at a time: each round the model gets the goal, the steps
done with their results and the VR screens by number with their apps now (`screens now: 1 main* ws2: foot | 2 left ws11: empty`, `*` = focused), and
answers the next single step from the catalogue (under the grammar) or `done`. At most 10 steps (the user's cap) or 30 s. Each step
shows "Step 2: Cinema mode" in the headset, the end one line with every step ("Done 3 steps: 1. Playing Dune.mkv | 2.
Cinema mode | 3. Background: black"). A failed step ends the goal ("Step 1 failed: No video called "dune" in
Downloads or Videos"); careful steps follow `voice_confirm`; only catalogue actions run (no shell). The model giving no
step at all (a weak model) sends the request the usual way (one decision or a plan of up to 4 steps). Online
(jev-router), the action model answers the steps. A part of the goal that is an exact catalogue phrase or an
apps-to-a-screen pattern ("terminals left", "chrome on screen 2", "swap screens 1 and 3", "the terminal from here to
screen 4") runs without asking the model, so a goal still works when the model is slow or down.

**Audit 2026-10-16 (evening)**, dry run, local Lara + Qwen3.5-4B, the same fixture phrases before (493d0ca) and after (plus each version's own catalogue phrases):
all 531/582 -> 617/625 (91% -> 99%); the natural phrasings of `tests/fixtures/audit.tsv` 256/294 -> 292/297; the goal set
`tests/fixtures/agent-2026-10-16.tsv` 2/17 -> 17/17; today's real phrases (`live-2026-10-16.tsv`) 53/55 -> 54/55. The
report: analysis/voice-audit.html.

**Agent model** (optional, ai.toml top level): `voice_agent_url = "http://127.0.0.1:8083/v1/chat/completions"` and
`voice_agent_model = "<alias>"` (OpenAI-compatible; a local llama-server also takes the step grammar;
`voice_agent_api_key_env` names an environment variable for a key). Then the steps of a goal go to that model (e.g. a
bigger local model on its own port), while simple commands stay on Lara + the 4B. If it fails or is not running, the
usual model goes on with the same goal. Unset: the usual model plans too.

**The tools and their inputs** (user request 2026-10-16: "the tools have to receive input"). Every catalogue intent is
a tool with typed inputs: `screen` (1-6, a side, or `here`), `side`, `app` (a kind or name, with "one"/"newest"), `n`
(a number, e.g. a volume percent), `q` / `text` (words), `name` (a layout or background), `how` (bigger, smaller,
closer, farther, left, right, up, down). The local model sees them as `id screen=1-6|left|...|here app="words"` lines
and a grammar that only lets those values through; an online model gets one JSON schema per tool. Every value is
checked again before anything runs (`command::slot_ok`, `windows::screen_value`). Online models also get
`move_apps {apps, which: all|one, to, from}` and `list_screens` (read-only). There is no free shell.

Examples: "Max, let's enter cinema mode, I want to watch Dune, it's already downloaded" (play Dune, cinema mode,
passthrough off), "Max, set up work: terminals left, browser middle, YouTube up-right", "Max, let's set up four
screens, the terminals on the left and Chrome on the right".

**Cancel** (user decision 2026-10-16): "Max, cancel", "Max, cancel that", "Max, stop", or "cancel"/"stop" alone while a
plan or a goal runs. `omarchyvr-voice act`'s listener thread watches for these while a task runs (`Actor::task_busy`): the
task stops before its next step (a step already started finishes and is not undone) and says "Cancelled after step 1 of 3" (in a goal, M = the parts heard); the words never reach the planner as a new request.

### Recording (docs/RECORD.md)

| say "Max ..." | does | intent |
|---|---|---|
| start recording / start a recording / record / record video / record this / start the recording / empieza a grabar / graba / grabar | `omarchyvr record start` with the saved settings (the toast shows its answer) | vr.record.start |
| stop recording / stop the recording / end recording / end the recording / finish recording / para de grabar / deja de grabar / termina la grabacion | `omarchyvr record stop`: saves the file, the toast says where | vr.record.stop |
| am i recording / are we recording / recording status / is it recording / estoy grabando | `omarchyvr record status` | vr.record.status |

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

## Checking every command (`omarchyvr-voice audit`)
The app's AI page has **Check all voice commands** (and **Open the last check**). It runs `omarchyvr-voice audit` at
nice 19: every intent's first phrase, the same phrase as the speech-to-text might mangle it ("work" -> "war",
"list" -> "liz"), and ~200 natural phrasings (`rust/omarchyvr-voice/tests/fixtures/audit.tsv`) go through the whole route
(exact, fuzzy, model) in a **dry run**: nothing is executed, typed or opened. It pauses 1 s after each model request
(`--pause-ms`) so the desktop stays responsive, and it refuses to start during a VR session (the live voice commands
need the model). Results: [AI.md](AI.md) section 13 and `analysis/voice-audit.html`.

`--live` (CLI only, for agents) also types for real into a scratch terminal the audit opens and closes itself
(`foot` running `cat`, closed by its PID): "type hello pimax", Enter, Backspace, checked in what `cat` received.
Keys are sent only while that window has the focus; browser, window and workspace commands stay in the dry run.

## Live session 2026-10-16: why most requests failed

Every "Max" request of the two Work sessions that afternoon (voiceact.log, ai.log traces, voice.log), 23 that reached
`act` plus 6 sentences that never did:

| cause | requests | examples |
|---|---|---|
| online AI unusable: OpenRouter HTTP 401 (key expired), the local model not yet on | 7 | "can you create 2 screens more", "do 2 more screens", "can you move the windows in a nice layout" |
| cut off: the end of speech came at a 0.5 s pause ("uh", "the") | 4 (+2 among the 401s, +3 lost halves) | "can you move uh 1 of the", "can you move all the", "can you write uh" (ran network info), "... something in the" (typed it) |
| misheard (speech-to-text) | 4 | "can i focus you 2" (focused a terminal as "user"), "canwii", "2 screams up there", "stop us through" |
| missing intent (screen count, app on a new screen) | 4 | "2 more screens" ran work-3 again (already 3), "can we have 4 screens" ran work-3 |
| needed several steps | 1 | "focus youtube in a new window" only focused YouTube |
| wrong intent | 1 | "focus in the main window" focused Chrome |
| "Max" not first, or the second half of a cut-off sentence | 6 | "Windows, can we ha Max, can we have two more windows?", "One of the screens to the right." |
| right | 3 (+2 near: work-3) | "open a new window", "open youtube", "clot screens" (work-1) |

No plan of several steps ran: the plan grammar needed "and"/"then" in the words, and none of the requests had it
(AI.md section 16). The fixes: the endpointing and the held cut-off sentence (Pauses, above), "Max" later in a sentence,
the mishearings (screams, clot, us through), app slots from the words heard, the screen intents and the 6 screens
(VOICE-DECISIONS 10-19), plans of up to 4 steps on more cues, and agent mode.

Dry-run audit (local Lara + Qwen3.5-4B on the GPU, `tests/fixtures/live-2026-10-16.tsv`):

| run | the 29 live requests | with variants (53) | full audit |
|---|---|---|---|
| before (3be30ba) | 13/29 (45 %) | 20/53 | 456/460 (99.1 %) |
| after (0ae6425 + fixes) | 27/29 pass, 1 asks ("Did you mean youtube?") | 51/53 | 476/483 (98.6 %) |

## How the local voice AI decides (scoped planner, user design 2026-10-16)

```text
 "Max, ..." ─► rules.rs ── screen numbers / stop / sizes / close everything ──► run (no model)
     │
     ├─► exact phrase / clear fuzzy hit ───────────────────────────────────────► run (no model)
     │
     ├─► Lara (~30 ms): the category (+ the runner-up when unsure) ─┐
     └─► fuzzy matcher: the 3 closest intents (hints) ──────────────┤
                                                                    ▼
        system prompt of THOSE categories (fixed text per set, cached by llama-server)
        + only their tools (10-25 of 136) + the hints + none / cancel / help
        + a grammar of exactly those tools
        user message: the state the category needs (world::describe: screens by number, shown or
        hidden, with their apps by kind and title; focus; gaze; tabs; session; view; media), hints, heard
                                                                    ▼
        model: [target: <the state the user wants>]  step ; step ...  (at most 4)
                                                                    ▼
        dry run on the state snapshot (world::apply, step by step): an unknown tool, a missing input, or a
        plan that closes an app nobody named is refused ──► run
```

- The question the model answers: "Using only these tools, which steps, in order and with which inputs, reach the
  state the user wants?" (`planner::system_prompt`).
- A request about screens ("close screen 2") never sees `browser.search` or the app closers; an answer outside the
  offered tools is refused even without a grammar.
- Each knob is a variant setting (`omarchyvr-voice audit --state --variant FILE`, `tests/state/variants/*.toml`):
  `tools.scope` = all (production for now) | category | category+hints | topk, `tools.top_k`, `prompt.blocks`,
  `prompt.target_line`, `prompt.template` (a file with another opening), plus the model, Lara and grammar knobs the
  runner applies. An experiment is a config change.
- Lara off or failing: the single prompt with every intent (the old route) answers.
- Goals of several steps (agent mode) keep their own loop (`agent::goal`).
- First measurement (2026-10-16, intent audit, dry run, Lara + Qwen3.5-4B, 298 natural phrasings): single prompt
  293/298 at p50 311-341 ms; scoped category+hints 280/298 at p50 655 ms. The second step of a plan across two
  categories ("cinema mode then dim the room") had no tool, and every category's prompt misses the one llama-server
  slot's cache. So production keeps the single prompt (with the screens line). The scoped planner is the subject of
  the experiment loop on the state audit, and wins only when it beats the single prompt on the held-out set.
- The audit page shows, under each phrase, exactly what the model got ("what the model got").
- State-audit loop (2026-10-16 evening, docs/VOICE-STATE-TESTS.md section 6): the planner now falls back to the
  single prompt when its answer fails or is `none` (`tools.fallback`). It still loses on dev and is slower (p95
  0.9-1.0 s against 0.45-0.6 s), so production keeps the single prompt.

## What decides before the model (general grammar, 2026-10-16)

The rules and the matcher settle about 3 of 4 requests alone (`--no-ai`: 72% of the state cases). All of it is
grammar, never a test phrase (`leak::tests` checks):
- **Clauses.** "close screen 2 and make the main one bigger": split at "and" / "then" before a verb; each clause is
  decided on its own (rule, exact phrase, else the model for that clause). A later clause that points back ("put
  Chrome there") keeps the whole request for the model.
- **Take-backs.** "X, no, forget it" / "X, cancel that" does nothing; "X, no, Y" runs Y (only when a pause is marked
  at the "no": "cinema mode, no room" is passthrough off).
- **Openers and stray words.** "yeah", "so", "okay" in front are dropped; one or two words the ASR put before an
  exact phrase ("sex, next background") are dropped when they are no command word.
- **Screens by side.** "hide the left screen", "unhide the upper one", "make the main one bigger", "move this to the
  right screen", "put the editors on the left one"; "move the screen up a bit" moves the screen itself; "where did
  my screens go" resets them; "screen for" = screen 4. A command that sends something to a hidden screen shows it;
  lifting the screen limit never lowers it.
- **Opening apps.** "open Google Chrome" shows the Chrome that is open (its hidden screen comes back); "open chrome
  here" / "in main" / "on screen 3" brings it there; a new window only when asked; a full desk never blocks it.
- **Closing (user rule: no app closes unless named).** `win.close.named`: "close Spotify", "close the video";
  "close YouTube" closes only that tab. Whatever picked it (rule, matcher, fuzzy ask-back, model), a destructive
  action runs only when the words ask for it: a close verb; window / workspace for a whole workspace; stop + VR /
  session words; type / write for typing; and a launcher only when the words name that app.

## Live session 2026-10-16 15:49-16:00: 3 of ~16 right

| request (heard) | ran | cause | fix |
|---|---|---|---|
| "Max, can you move this screen number one to the right?" (the first) | nothing, not logged | `act` starts ~12 s after the listener (the launcher's settle waits); the final was sent before it had subscribed | the listener keeps the finals said before the first command client; `act` asks for them once (`backlog`, max 30 s old) |
| "Can you move it in front of me, Max, the screen." | app fullscreen | only "the screen" (after "Max") reached the model | the words before "Max" stay when the rest is a fragment; "in front of me" = vr.screens.reset |
| "make all these screens half the size" (x2) | screen 1 one step smaller | no intent for all screens / a size | win.screen.scale (all, half, double, percent) |
| "remove screen number four and three" (x2) | swap 4 and 3 | no "hide screen" intent; the model picked the one with two screen slots | vr.screens.hide with a list of numbers; rule before the model |
| "Can you Max can you shut down numbers screens four and three" | nothing, not logged | "Max" not first and no comma around it: never woke | "Max" after fillers / before "can you" wakes |
| "move all the terminals to screen number three" (x2) | gather on the LEFT screen | the router's top was put_n (0.83-0.90); the model chose gather and dropped screen=3 ("screen number 3" matched no phrase) | "screen number N" = "screen N"; app + "to screen N" = put_n by rule |
| "close (down) screen number two" (x4) | win.ws.close on a gazed workspace; once "Close window 1 (2 apps)?" | the model's "close window" for a screen; the target was re-read at the yes | rule: hide the screen; close window never for a screen; pinned workspace |
| "shut down work VR", "can you stop the VR" | nothing | no stop intent | vr.session.stop (asks yes) |
| "close everything" (x2) | nothing | no intent | vr.screens.hide_others |
| (all session) | Parakeet on the CPU | the GPU speech server (8082) had been stopped at 14:59 and nothing started it again: `ai.server`'s health only watches llama-server, and game mode's restore ran `omarchyvr-ai serve` (the action model only) | Work start runs `omarchyvr-ai up --wait 0` (detached) when Local AI is on; game mode restores with `omarchyvr-ai up` |

Audit (dry run, local Lara + Qwen3.5-4B on the GPU; the natural phrasings of each set, before 2068884 -> after
7a3181e; "all" adds each version's own catalogue phrases, exact and ASR-mangled):

| set | natural before | natural after | all before | all after |
|---|---|---|---|---|
| this session (`live-2026-10-16-b.tsv`, new) | 9/36 | 36/36 | 271/300 | 304/308 |
| goals, long (`agent27b-2026-10-16.tsv`) | 3/12 | 12/12 | 265/276 | 280/284 |
| goals (`agent-2026-10-16.tsv`) | 15/15 | 15/15 | 277/279 | 283/287 |
| audit.tsv | 287/293 | 293/298 | 549/557 | 561/570 |
| live 2026-10-16 morning | 52/53 | 51/53 | 314/317 | 319/325 |
| aibench | 41/45 | 42/45 | 303/309 | 310/317 |

The intent audit is now a smoke test: the measure of quality is the state audit (`omarchyvr-voice audit --state`,
docs/VOICE-STATE-TESTS.md), which checks the end state of the desktop, not the intent name.

## Your own changes: `~/.config/omarchyvr/voice-intents.toml`
This file is optional and not created for you. It uses the same format as the built-in catalogue (the comments at the
top of `rust/omarchyvr-voice/voice-intents.toml` explain each key).
- An `[[intent]]` with an existing `id` sets `enabled`, adds `phrases`, and can replace `run`, `slots` and `say`.
- A new `id` adds a command.
- `[wake]`, `[paths]`, `[backdrop]` and `[llm]` replace the defaults.

```toml
# ~/.config/omarchyvr/voice-intents.toml
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
# api_key_file = "~/.config/omarchyvr/llm.key"   (or api_key_env = "OPENAI_API_KEY"); never put a key in the repo
# backend = "off"
timeout_s = 6.0
```
Remote calls go through `curl -K -` (the key is on stdin, not on the command line). Nothing is installed for you. For
local ollama: `ollama pull qwen2.5:3b`.

## AI agent (voice_ai)
With `voice_ai = "fallback"` or `"always"` in `~/.config/omarchyvr/ai.toml` (the app's AI tab), a model with tools handles
what the matcher does not understand, and deictic requests ("Max, close this", "the one I'm looking at") use the gaze.
When the agent is on, it replaces the `[llm]` fallback and the AI switch below for those requests. The tools are the
catalogue's own intents, so `confirm` intents still need "yes" and `never` rows are refused. Local model:
`omarchyvr-ai install` / `serve --cpu` (in VR). The local model gets a compact request (the intent list once, cached;
per command only the fuzzy hints, the words and a little context) and answers in about 1.3 s on the CPU.
`omarchyvr-voice aibench` measures it. Everything else is in [AI.md](AI.md).

## AI switch (optional)
For requests the offline matcher does not know ("Max, go to the window where I was reading about Monado"), or when no
window matches the name you said, a model can pick the window.
- **Setup:** a provider in `~/.config/omarchyvr/ai.toml` (the GUI's AI tab writes it). No file or no provider = off.
  `omarchyvr-voice act` logs "AI switch: ..." at start. Interim format, read by `llm::ChatClient`:
  ```toml
  [switch]
  kind = "anthropic"                 # anthropic | openai | openai-compatible
  # model = "claude-haiku-5-5"       # default for anthropic; required for openai kinds
  # api_key_file = "~/.config/omarchyvr/anthropic.key"   (or api_key_env; default ANTHROPIC_API_KEY / OPENAI_API_KEY)
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
- **In desk mode:** `omarchyvr desk start --voice` starts three processes.
  - `omarchyvr-voice listen` (mic to text) and `omarchyvr-voice captions` (captions panel).
  - **`omarchyvr-voice act`** (commands): log `voiceact.log` in the session dir, undo state in
    `$XDG_RUNTIME_DIR/omarchyvr-voice-act.state`.
- **Text instead of speech:**
  - `omarchyvr-voice do "pimax cinema mode" --dry-run` prints what would happen.
  - `omarchyvr-voice do "pimax lock the screen" "yes" --dry-run` tests a confirmation.
  - `--no-wake` implies "Max".
- **Listen without acting:** `omarchyvr-voice act --dry-run` follows a running listener and prints the actions instead of
  doing them.
- **List:** `omarchyvr-voice intents [--all]`.
- **Passthrough by hand:** `omarchyvr-passthrough ctl on|off|dim|normal|brighter|darker|gain 0.5|status`.
  - The control socket is `$XDG_RUNTIME_DIR/omarchyvr-passthrough.sock`.
- **WayVR by hand:**
  - `wayvrctl desk-layout 3 --curvature 0 --width 1.6 --distance 2`;
  - `wayvrctl skybox off|default|skybox/NAME.dds`;
  - `wayvrctl recenter`.

### What needs a rebuild or restart (do it with desk mode stopped)
```sh
omarchyvr desk stop
cd ~/omarchyvr/rust && cargo build --release -p omarchyvr-voice -p omarchyvr-passthrough -p omarchyvr-tools
install -m755 target/release/omarchyvr ~/.local/bin/omarchyvr      # the launcher (desk preset, cinema default, voiceact)
cp $WAYVR_TARGET/release/wayvr $WAYVR_TARGET/release/wayvrctl ~/.local/lib/omarchyvr/wayvr/
omarchyvr desk start --voice
```
The launcher runs `omarchyvr-voice` and `omarchyvr-passthrough` from `~/omarchyvr/rust/target/release`, so building them is
enough.
Older parts keep working:
- An older WayVR (before `aac483a`) has no `desk-layout` or `skybox`. Presets then only change the passthrough and
  `60-preset.yaml`, and backgrounds other than passthrough/dimmed/black report an error toast.
- An older passthrough has no control socket.

Live test steps: [analysis/voice-live-checklist.md](../analysis/voice-live-checklist.md).

## GUI integration (for omarchyvr-app / omarchyvr-gui-core; not done here)
A **Voice** settings row could have:
- **Voice commands** on/off: start desk with `--voice`, or not.
- **Wake word**: edit `[wake] words` in `~/.config/omarchyvr/voice-intents.toml`. Show the active list from
  `omarchyvr-voice intents` (the "wake words:" line).
- **LLM backend**: off / auto (local ollama) / ollama model / OpenAI-compatible (url, model, key file). Write the
  `[llm]` table. Never store the key in the table itself; point `api_key_file` at a 0600 file.
- **Intent list with toggles**:
  - Fill it from `omarchyvr-voice intents --all`, one line per intent: `id [safety, source, confirm] phrase | phrase`,
    with `(off)` in front when disabled.
  - A toggle writes `[[intent]] id = "..." enabled = true|false` to the override file.
  - Do not offer toggles for rows that are not approved: they are not in the list at all.
- **Preset picker**: cinema (default) / huge cinema / work-3 / work-1 / assistant / none.
  - The GUI today omits `--preset` for "none". That now means **cinema**, so pass `--preset none` explicitly for no
    preset.
  - While desk mode runs, "Apply now" runs `omarchyvr desk preset NAME`.
- **Work panels**: two text fields for `left =` / `right =` in `~/.config/omarchyvr/work-panels.conf`.

## Speech engine (2026-10-16)

`omarchyvr-voice listen` recognises speech with Parakeet TDT v3 on the processor (biased to words like Max, passthrough,
screens, workspace, YouTube). This is the default (since 2026-10-16). Only when the switch "Speech on graphics card"
on the Local AI page is On (`asr = "gpu"` in ai.toml, default Off) and the local models run (Work), it uses Whisper
large-v3-turbo on the graphics card through `whisper-server` (port 8082); the GPU server shared the card with the AI
models and made Monado skip 8.8% of its frames. If the server is down, Parakeet answers, so voice never stops. The
engine in use is the first line of voice.log. Details and numbers: docs/AI.md sections 17 and 19.
