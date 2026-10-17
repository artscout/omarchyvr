# Play and Work: audit (2026-10-17)

Two questions of the user: is the whole graphics card free for the game in Play, and does Exit Work stop everything
of Work? Evidence is `file:line` at omarchyvr `8c69b54` (WayVR fork `bdda5a63`) unless it says "now". Section 3 lists
what this change fixed.

## 1. Play and video memory (A1)

### What can run while Play runs

| Process or allocation | GPU memory | In Play? | Who starts / stops it |
|---|---|---|---|
| monado-service + Pimax driver (the compositor) | yes | always | `vr start` / `vr stop` (vr/session.rs) |
| SLAM / Basalt (inside Monado) + `pimax-cam broker` | no (CPU) | with 6DoF (default on) | `sixdof::vr_start`, `stop_broker` |
| controller tracking `pimax-ctrltrack` | no (CPU) | with optical tracking (default on) | `sixdof::vr_start`, `stop_ctrltrack` |
| camera hands (Mercury, ONNX Runtime CPU, in Monado) | no | only when switched on | `--hands` |
| eye tracking (`PIMAX_EYE`, in Monado) | no | base: on without Light game mode (vr/mod.rs:393) | `--eye` / `--no-eye` |
| wrist readout: a 2nd WayVR `--headless` + `omarchyvr-perf` | small (overlay) | Light game mode off + `--perf` | perf::vr_start / vr_stop_clients |
| SteamVR (vrserver, vrcompositor) | yes | "Games run with: SteamVR" | steamvr::start / stop |
| room search `omarchyvr-roommap auto` | no | Light game mode off | room::on_start / on_stop |
| recorder | encoder | only while recording | omarchyvr-record |
| the app (egui, OpenGL) | a small context | always | the user |
| **local AI model** llama-server (Vulkan) | about 3 GB (4B), 4.8 GB (9B) | **could survive** | omarchyvr-ai up / down |
| **agent model** llama-server (27B, opt-in) | about 16.7 GB | **could survive** | omarchyvr-ai up / down |
| **whisper-server** (GPU speech, opt-in) | about 0.9 GB | **could survive** | omarchyvr-ai up / down |
| **Lara** laya-serve (PyTorch CPU) | no (about 2 GB RAM) | **could survive** | omarchyvr-ai up / down |
| **VRAM watch** `omarchyvr-ai watch` | no | **could survive** | omarchyvr-ai up / down |
| Lara tuning `omarchyvr-ai tune-lara` (once, about 1 h, nice 19) | no (CPU) | can run | started after Lara's download |
| voice (`omarchyvr-voice` listen / captions / act, voice HUD; Parakeet on the CPU) | no | not started by `vr start` | desk start / stop |
| WayVR desk, passthrough | yes | not started by `vr start` | desk start / stop |

Speech already runs on the processor by default in omarchyvr (`Asr::Cpu` is the default, omarchyvr-config ai.rs:313-320,
docs/AI.md section 19): the decision of lab commit 5174fc8 is in; nothing to change.

### How Play started (base)

Play (home.rs) -> `AppCore::play` (play.rs:185) -> `vr_start_job` (app.rs:1137) -> `omarchyvr vr start` with
`launcher::vr_game_args` (launcher.rs:332) -> vr/mod.rs `start`. The local AI was touched only after Monado was up:
`gamemode::on_start` (vr/mod.rs:446, 467, 503) ran `omarchyvr-ai stop` (gamemode.rs:196), which stops the main slot
only (omarchyvr-ai main.rs:229-240), and only with "Game mode" (`pause_ai`) on. At `vr stop`, `gamemode::on_stop`
(vr/mod.rs:588; gamemode.rs:245-274) started the AI again (`omarchyvr-ai up`), at the desktop.

### Work to Play (base)

Play was greyed out while Work ran (home.rs:236: "Stop the running session first"). Stop Work is `omarchyvr desk stop
--grace 30` (app.rs:1091), and that stop (desk.rs:785-874) had no AI step, while Work had started the AI
(desk.rs:1139, `local_ai_up`). So the AI of Work was still loaded when Play started.

### omarchyvrd (base)

Its rule `ai(d) = d.ai_server && desk(d)` (components spec.rs:110-112) never starts the AI in a game session, but the
reconciler only starts what is wanted (reconcile.rs:142, 287): a running ai.server stops only on session off or a
toggle off; an override turns it on in any mode (state.rs:202-209); `voice.listen` and `voicehud` had no desk gate
(spec.rs:99-107). omarchyvrd is observe-only by default (state.rs:181), so the launcher decides in practice.

### Other ways an AI process could run in Play (base)

- The AI page's Start / Up buttons (gui-core localai.rs:339-362) and `omarchyvr-ai up` / `serve` (twostage.rs:547,
  main.rs:133) had no Play check; the app called GPU mode next to a session "allowed" (localai.rs:405).
- `omarchyvr-ai up` loaded the models whenever `voice_local_models` was not explicitly false (twostage.rs:548).
- The VRAM watch (twostage.rs:761) only shrinks a model next to a game; it does not stop it.
- `omarchyvr-ai.service` is installed, not enabled (installer/install.sh:398-404).

### Verdict (base): not true

"All video memory free for the game in Play" was **not guaranteed**:

| # | Gap |
|---|---|
| G1 | Work's stop left the local AI loaded (desk.rs:785-874). |
| G2 | Play's pause stopped only the model slot: whisper-server, the 27B agent model and Lara survived (gamemode.rs:196, main.rs:229-240). |
| G3 | The pause ran after Monado started, and only with a switch that could be off (`pause_ai`). |
| G4 | `vr stop` started the AI again, outside Work (gamemode.rs:245-274). |
| G5 | Nothing refused an AI start during Play (omarchyvr-ai up / serve, the AI page, an omarchyvrd toggle). |
| G6 | omarchyvrd allowed voice.listen / voicehud in a game session and never stopped a running ai.server on a switch to Play. |
| G7 | Eye tracking in Play followed Light game mode (off with it, on by default without it; vr/mod.rs:393), not its own switch. |

## 2. Exit Work (A2)

### What the headset offered (WayVR fork `bdda5a63`)

- The watch (left wrist, wayvr/src/assets/gui/watch.xml): keyboard, edit mode, recenter, fix floor, dashboard, the
  overlay toggles, the hidden Local AI row. **No exit.** `::Shutdown` exists (gui/panel/button.rs:649-655) but no layout
  uses it, and it would end only WayVR, not Monado, voice or the AI.
- Controller: left menu click = play-space drag, double click = play-space reset (wlx-common/assets/
  openxr_actions.json5:117-122); left Y double click = show / hide the screens. **No menu.**
- Bare hands: pinch only (backend/hand_pointer.rs:107-118); the watch follows the left hand (overlays/watch.rs:134-137).
- WayVR's dashboard: no exit ("Restart software" only).
- Watch buttons run `sh -c` (button.rs:819-847) with WayVR's isolated XDG_*_HOME (components wayvr.rs:31-46): the
  Local AI buttons' `omarchyvr-voice ai-mode` read and wrote a second ai.toml in the WayVR home (voice main.rs:828,
  config ai.rs:899). A bug.

### What a stop stopped (base)

| Work part | `desk stop` (the app's Stop) |
|---|---|
| voice listen / captions / act, voice HUD, screens watcher, wrist readout | yes, SIGINT by pid file, name checked (desk.rs:386-408) |
| WayVR | yes, SIGINT, waits; hung: SIGTERM / SIGKILL once Monado is gone |
| passthrough, controller tracking, recorder, room search | yes |
| Monado | yes, SIGINT, SIGKILL only after the grace (user stops) |
| camera broker | yes, last |
| VR screens (Hyprland headless outputs PIMAX-*) | yes, once no Monado runs (desk.rs:868; not in the watchdog) |
| audio routing | restored (desk.rs:1102) |
| omarchyvrd | told session off |
| **local AI: model, Lara, whisper-server, agent model, VRAM watch** | **no** |
| Lara tuning (CPU, once) | no |
| locks | start.lock / stop.lock are flocks (released at exit); the user-stop marker is written; start-argv.txt stays on purpose (it repeats the last session) |

A crash of the launcher or the app left the same parts running until the next stop or start; nothing cleared a crashed
Work's leftovers before Play.

| # | Gap |
|---|---|
| E1 | No Exit in the headset (no menu, nothing on the watch). |
| E2 | The local AI survived every stop (app Stop, a switch to Play). |
| E3 | A crashed Work's leftovers (voice, WayVR, the AI, the VR screens) were not cleared before Play. |
| E4 | Commands run from WayVR's buttons used WayVR's XDG directories. |

## 3. What this change did

| Gap | Fix (now) |
|---|---|
| G1, E2 | `desk stop` is the one full stop of Work: after the clients it stops every `omarchyvr-ai` slot and the VRAM watch (vr/work.rs `stop_ai`, through `omarchyvr_ai::twostage::down`: pid files, command line checked), then names whatever of ours still runs (`work::survivors`) as a problem. A restart keeps the AI (`--keep-ai`, the watchdog). The app's Stop, Exit Work and the switch to Play all use it. |
| G2, G3, E3 | `vr start` clears before it starts anything (vr/work.rs `clear_for_play`, every start path): when no monado-service runs, a crashed Work's clients get the desk's own stop by pid, every AI slot stops, leftover PIMAX-* outputs go; a running session is never touched. No switch: always. What it stopped is shown on Home ("Stopped for Play: local AI, voice"). |
| G4 | `vr stop` starts nothing of Work again (gamemode.rs `on_stop`); the AI comes back with the next Work. `pause_ai` is read from old files but has no effect. |
| G5 | `omarchyvr-ai up` and `serve` refuse while Play runs (`twostage::play_running`: start-argv says `vr` and its monado-service is alive, or SteamVR's vrserver; `--force` for tests); `up` loads only what ai.toml wants (`ai_servers_wanted`); the AI page says Play refuses it. |
| G6 | omarchyvrd: `WORK_ONLY` parts (ai.server, voice.listen, voice.captions, voice.act, voicehud) are never wanted in a game session, whatever an override says (state.rs `wanted`), voice is desk-gated (spec.rs), and the reconciler stops any of them it finds running in one (reconcile.rs). |
| G7 | "Eye tracking in games" (gamemode.toml `eye_in_games`, default off; Home and Display; `omarchyvr vr gamemode eye on|off`): `vr start` resolves `--eye` / `--no-eye` from it and records the choice in start-argv; Light game mode no longer decides eye tracking; a Play start no longer changes Work's eye switch. 6DoF, the controllers and their optical tracking stay on in Play. |
| E1 | WayVR (fork commit `e9189c00`): a `menu` binding (Touch: left menu click) opens the Work menu panel named by `menu_panel` in front of you; the launcher writes that panel (`work_menu`: Exit Work, Close menu, Recenter, WayVR settings) and `menu_panel: work_menu`. The watch has an always-visible Exit Work button (the way with bare hands). Both run `omarchyvr desk exit`: the first press arms (a toast "press again"), the second starts `omarchyvr desk stop --grace 30` detached from WayVR (log `<session dir>/exit.log`). |
| E4 | The launcher hands the user's own XDG directories to WayVR as `OMARCHYVR_USER_XDG_*` and puts the tools first on its PATH; `omarchyvr`, `omarchyvr-voice` and `omarchyvr-ai` put them back first thing (`omarchyvr_config::paths::restore_user_xdg`). |
| check | Home shows, while Play runs, "Graphics card: nothing of Work or the AI uses it: 17.0 of 24.0 GB free" or names what is left (gui-core vram.rs: `nvidia-smi --query-compute-apps` / `--query-gpu`, read-only, plus the AI's pid files; no GPU work). |

Tests (fake processes named like the real binaries, the leaf_restart pattern): vr/work.rs `exit_work_stops_every_tracked_component_and_nothing_else`,
`a_restart_keeps_the_local_ai_loaded`, `play_clears_a_crashed_works_leftovers_and_the_ai_but_never_a_running_session`,
`exit_work_asks_for_a_second_press_and_only_in_work`; omarchyvr-ai `play_is_seen_from_the_session_record_or_steamvr`;
components spec `default_desired_wants_the_desk_set` (Work-only parts never in Play); gui-core play.rs
`switching_work_to_play_is_the_full_stop_of_work_then_play_in_one_job`, `play_keeps_tracking_and_eye_tracking_follows_its_own_switch`, vram.rs.

## 4. Still open

- Lara's one-time tuning (CPU only, nice 19, about an hour after its download) is not stopped by Play: no video memory,
  but processor time. Stopping it loses the run (it starts over).
- No hand gesture opens the Work menu; with bare hands the watch's Exit Work is the way (the menu button is a
  controller button).
- An AI server started by hand with `omarchyvr-ai up --force` during Play is the user's own choice; Home names it.
- omarchyvrd's own stops of Work (an unplug, an automatic switch) run in the watchdog and keep the AI loaded; with no
  Work left, `ai.server` is no longer wanted, and a switch to Play still frees it at the `vr start`.
- Speech on the graphics card runs only next to the local model (its server is started with it); the Work questions
  offer it only with "A model on this PC".
- Not tried in the headset yet (no VR session was started for this audit).
