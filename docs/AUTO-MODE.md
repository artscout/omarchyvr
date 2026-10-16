# Auto mode (`omarchyvr-autod`)

Put the headset on and the session starts. Leave it and the session stops. A small user daemon does this:
- it is ~700 KB stripped and uses ~2 MB RSS;
- it uses no CPU at idle (no headset);
- it has no async runtime and only `omarchyvr`, `pimax-proto` and `libc` as dependencies.

**It runs in dry-run mode until you set `dry_run = false` in `~/.config/omarchyvr/auto.toml`.** In dry-run it logs the
commands it would run and changes nothing.

Background and signal list: [analysis/auto-mode.md](../analysis/auto-mode.md). Live test:
[analysis/auto-mode-live-checklist.md](../analysis/auto-mode-live-checklist.md).

## What it does
```
Unplugged -> Plugged -> Worn (start) -> Active(desk|game) -> IdleDim -> Sleep (session stopped)
                ^-------------------- put on again -------------------------/
```
- **Plugged:** USB 34a4:0044 is seen for 4 s, so a power-cycle re-enumeration does not count.
- **Worn:** the IMU shows a head's micro-motion for 4 s continuously. A pick-up-and-put-down does not start anything.
  Controller input also counts as wear evidence.
- **Start**, from config:
  - default `omarchyvr desk start --6dof --ctrl-track --no-notify`, with the perf readout on (it is the desk default);
  - optional `--preset`;
  - in game mode `omarchyvr vr start --6dof --ctrl-track --perf`.
  - If the room map `home` exists, Monado gets `PIMAX_ANCHOR=1`, followed by
    `omarchyvr-roommap relocalize --source broker --name home --apply`.
  - The IPD from `~/.config/omarchyvr/ipd.env` reaches Monado through the launcher, which now reads it itself.
- **IdleDim:** the headset is not worn (IMU still) for 60 s. You get a notification. Wearing it again for 4 s returns
  to Active.
- **Sleep:** IdleDim lasts another 600 s, then auto mode runs `omarchyvr desk stop --grace 30` (or `omarchyvr vr stop --grace
  30`). The launcher stops processes by pid. Displays and DRM are never touched.
- **Games are never stopped** while a Steam game (`SteamLaunch AppId=`), SteamVR or a main OpenXR app in a non-desk
  session runs, unless `sleep_games = true`.
- **Sessions you started by hand** are shown as Active but never stopped (`sleep_foreign = false`).
- **Closing a session yourself while wearing the headset** does not restart it. Take the headset off and put it on
  again to restart.
- **A failed start** is retried only after you take the headset off, put it on again, and `retry_after_s` (120 s) has
  passed.
- **Unplugging** (6 s) stops a session that auto mode started.
- **An IMU that reports nothing** (Unknown) never makes the session idle.
- **Exiting the daemon** never stops a session.

## Setup
```sh
cd ~/omarchyvr/rust && cargo build --release -p omarchyvr-autod -p omarchyvr-tools
cp ../packaging/auto.toml.example ~/.config/omarchyvr/auto.toml         # defaults; dry_run = true
target/release/omarchyvr-autod config                                    # effective settings + warnings
target/release/omarchyvr-autod run                                       # foreground, log on stderr
# later, as a user service (not installed by anything):
install -Dm644 ../packaging/systemd/omarchyvr-autod.service ~/.config/systemd/user/omarchyvr-autod.service
```
Install `omarchyvr` (with `--preset` and `omarchyvr auto`) only while nothing runs (docs/LESSONS.md row 8).

## Config (`~/.config/omarchyvr/auto.toml`)
All keys and defaults are listed in [packaging/auto.toml.example](../packaging/auto.toml.example). The main ones:

| key | default | meaning |
|---|---|---|
| `dry_run` | true | log only |
| `auto_start` | true | start on put-on |
| `mode` | desk | desk or game |
| `game_runtime` | monado | monado (Monado + xrizer) or steamvr. **SteamVR is phase 1 (static):** auto mode starts nothing for it, notifies once and keeps out of its way. `omarchyvr vr steam-options` prints `%command%` for steamvr. |
| `preset` | none | none (= the launcher default, cinema), cinema, work-3, work-1, assistant (below) |
| `sixdof`, `ctrl_track`, `perf` | true | session flags |
| `relocalize`, `room_map` | true, home | room relocalisation after a start (when the map exists) |
| `sleep_games`, `sleep_foreign` | false | allow stopping games / sessions not started by auto mode |
| `wear_on_s`, `dim_after_s`, `sleep_after_s` | 4, 60, 600 | timing |
| `still_dps`, `worn_dps` | 0.2, 0.3 | IMU gyro std thresholds (hysteresis between) |
| `imu` | true | read IF2/IF1 read-only for 0.4 s every `poll_s` while plugged |

## Desk presets (`omarchyvr desk start --preset NAME`, also usable without auto mode)
`omarchyvr desk start` uses **cinema** unless `--preset` says otherwise; `--preset none` removes the preset file. Each start
writes `<wayvr home>/config/wayvr/conf.d/60-preset.yaml`. It sorts after the launcher's files and the app's
`50-pimax-app.yaml`, so the preset wins. Once WayVR runs, the preset's screen layout is applied over `wayvrctl`. Switch
live with `omarchyvr desk preset NAME` or by voice ([VOICE.md](VOICE.md)).

| preset | screens (live layout) | conf.d | launcher |
|---|---|---|---|
| cinema (default) | 1 big curved | `default_curvature: 0.35` | passthrough dimmed: `--gain 0.35` |
| work-3 (alias work) | desktop + 2 WayVR app panels, flat | `default_curvature: 0.0`, `screen_max_height: 2160` | - |
| work-1 | 1 flat | same as work-3 | - |
| assistant | all, slightly curved | `default_curvature: 0.15` | `--voice` (captions + commands) |

## Control: `omarchyvr auto ...` / `omarchyvr-autod ...`
```
omarchyvr auto status                  one JSON line
omarchyvr auto events                  stream (status first, then transitions and actions)
omarchyvr auto pause | resume          pause: no starts/stops; states are still tracked
omarchyvr auto wake                    "I'm wearing it": wear evidence for wear_on_s (start without IMU)
omarchyvr auto sleep                   stop now (own, non-game session)
omarchyvr auto set-mode game --runtime monado      restarts our own session to apply (never a running game)
omarchyvr auto set-mode --preset cinema
```
`set-mode` changes are not written back to auto.toml; they last until the daemon restarts.

## GUI integration (for omarchyvr-app / omarchyvr-gui-core)
The GUI does not link the daemon. It talks to the socket, or calls `omarchyvr auto`. Rust code may use
`omarchyvr_autod::client::{request, events, request_line}` and `omarchyvr_autod::ipc::socket_path()`.

- **Socket:** `$PIMAX_AUTOD_SOCKET`, else `$XDG_RUNTIME_DIR/omarchyvr-autod.sock`. It is a stream socket carrying one JSON
  object per line. Send a request line and read one reply line. If the socket is missing or the connect fails, the
  daemon is not running: offer `systemctl --user start omarchyvr-autod`, or show the run command.
- **Requests:** `{"cmd":"status"}`, `{"cmd":"pause"}`, `{"cmd":"resume"}`, `{"cmd":"wake"}`, `{"cmd":"sleep"}`,
  `{"cmd":"set-mode","mode":"desk|game","runtime":"monado|steamvr","preset":"none|cinema|work|assistant"}` (any
  subset of the last three), `{"cmd":"events"}`, `{"cmd":"pair","side":"left|right"}`.
- **Reply to every command except `events`:** the status object, or `{"ok":false,"error":"..."}`.
  ```json
  {"ok":true,"state":"active(desk)","paused":false,"dry_run":true,"owned":true,"busy":null,"uptime_s":812,
   "signals":{"plugged":true,"motion":"worn","visor":null,"session":"desk","game":false,"input":false,"worn":true},
   "config":{"mode":"desk","game_runtime":"monado","preset":"none","auto_start":true,"sleep_games":false,
             "dim_after_s":60,"sleep_after_s":600}}
  ```
  - `state` is one of `unplugged`, `plugged`, `worn`, `active(desk|game)`, `idle-dim(desk|game)`, `sleep`.
  - `busy` is `"start"`, `"stop"`, `"switch"` or null.
  - `motion` is one of `unknown`, `still`, `worn`, `moving`.
- **`events`:** the status object first, then one line per event until the client closes the connection.
  ```json
  {"event":"transition","t":812.0,"from":"plugged","to":"worn","why":"headset put on"}
  {"event":"action","t":812.0,"action":"start","detail":"Start { mode: Desk, ... }","dry_run":true}
  ```
- **Suggested GUI:**
  - a status pill (state + "dry run" badge);
  - a Pause/Resume toggle;
  - a mode switch (Desk / Game) with a runtime choice (Monado / SteamVR, the latter marked "phase 1: start SteamVR
    yourself");
  - a preset picker;
  - a "Start now" button that sends `wake`;
  - a "Sleep now" button that sends `sleep`;
  - **Pair controller** (StartPair approved by the user on 2026-10-14): a "Pair left" / "Pair right" button. On click
    send `{"cmd":"pair","side":"left"}` (reply `{"ok":true,"pairing":"started","side":"left"}`, or `{"ok":false,...}`
    if one is already running) and read the `events` stream. Show "Press and hold the controller's pairing buttons
    (see Pimax manual)" and a 35 s countdown while `waiting`. Events:
    ```json
    {"event":"pair","side":"left","state":"waiting","detail":"0/35 s: Press and hold ..."}   (at start, then every 5 s)
    {"event":"pair","side":"left","state":"paired","detail":"..."}
    {"event":"pair","side":"left","state":"timeout","detail":"..."}   (35 s timer or firmware cancel)
    {"event":"pair","side":"left","state":"failed","detail":"..."}    (could not send / firmware log unreadable: nothing sent)
    ```
    `paired`, `timeout` and `failed` are final. Exactly one StartPair is sent per click and it is never retried; the
    user presses the button again to retry. The send is the IPD-motor path (write-only IF0 write, coexists with a
    running Monado); a paired controller is attached by Monado's attach thread within about 2 s. Works in dry run too
    (it is an explicit user action). The CLI is `omarchyvr-autod pair left|right` / `omarchyvr auto pair left|right`
    (exit 1 unless paired; works without the daemon too).
- **Stopping sessions:** the GUI keeps its own buttons, which run the same `omarchyvr desk|vr stop`. Auto mode adopts the
  result: it sees the session gone, and does not restart until the headset is taken off and put on again.

## Simulator and replays (offline)
```sh
omarchyvr-autod sim rust/omarchyvr-autod/sim/*.sim     # put-on, take-off, idle-sleep, game-running, unplug, failed-start
omarchyvr-autod replay-imu ~/.local/share/omarchyvr/slam-recordings/<stamp>-live/euroc_*/mav0/imu0/data.csv
```
The format of a sim script is described at the top of `rust/omarchyvr-autod/src/sim.rs`. The `expect T STATE` lines are
checked, and `cargo test -p omarchyvr-autod` runs all the shipped scripts.

## Not done / needs approval
- **Visor 0x2c:** `omarchyvr-autod visor-probe` only. It sends the exact Windows read and is used for the live test. The
  daemon never sends it, and the reply has been constant in every capture.
- **Pairing send:** compiled out, needs approval.
- **Panel off on idle (0x29):** not sent; it is not approved.

## Unpair (controllers)
`omarchyvr controllers unpair left|right` sends the one captured command `58 03 05 SS` (SS 2 left, 3 right). Both hands are
captured: right in the Windows capture 2026-10-15 take 13, left and right again in the redo take 3
(`58 03 05 02` / `58 03 05 03`, replies `4c 04 05 SS 01 07`, the controller stream stops 20 ms later; analysis/windows-capture-20261015.md, Redo).
Write-only on the command interface, one write per request, refused while a pairing runs. The app has an Unpair button next to
Pair on each controller card, for both hands (one-line confirm, result in plain words). Pairing guidance: put the controller in
pairing mode first, then press Pair for that hand only and wait up to 60 s; the headset briefly drops the other controller while
it listens. A controller that was just unpaired and is still awake reconnects on the next Pair at once (20 ms in the redo take).
Pair and unpair are also refused while Monado or SteamVR owns the headset.
