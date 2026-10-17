# omarchyvrd: the Pimax stack's owner

`omarchyvrd` (the same program as `omarchyvr-autod`) is auto mode plus a reconciler over the component model in
`rust/omarchyvr-components`. The design is in [RESILIENCE-PLAN.md](RESILIENCE-PLAN.md) and
[../analysis/resilience-plan.html](../analysis/resilience-plan.html).

- **Observe only by default.** omarchyvrd probes, adopts running sessions, and publishes the state. It executes nothing until
  `[restart] enabled = true` is set in `~/.config/omarchyvr/omarchyvrd.toml`.
- **Safe mode.** omarchyvrd also executes nothing while `~/.config/omarchyvr/safe-mode` exists, or while it has put itself into
  safe mode. That happens when the global budget is used up, or when omarchyvrd restarts more than 3 times in 5 min.
- **Restarting omarchyvrd never stops a session.**

## Commands

```
omarchyvrd run [--config AUTO.toml] [--live|--dry-run]   # the daemon (packaging/systemd/omarchyvrd.service)
omarchyvrd health [--json] [--deep] [--probe] [--watch]   # the table; same as omarchyvr-health (read-only)
omarchyvrd state                                          # the snapshot below, from the running daemon
omarchyvrd set session=on|off [mode=desk|vr] [voice=on ...]
omarchyvrd toggle KEY on|off | fix ID | safe-mode on|off | ack-gpu
omarchyvrd wait overall=ok|health:monado=ok|intent=off [--timeout S]
omarchyvrd scenario rust/omarchyvr-autod/sim/scenarios/*.scn  # reconciler simulations (one per plan scenario)
omarchyvrd sim rust/omarchyvr-autod/sim/*.sim                 # auto-mode simulations (unchanged)
```

`omarchyvr-health` exits with 0 for ok, 1 for degraded or down, and 2 for needs-you or needs-reboot.

## Files

| Path | What |
|---|---|
| `~/.config/omarchyvr/omarchyvrd.toml` | The settings: `[session]`, `[components]`, `[restart]`, `[notify]` and `[auto]`. The full example is in `rust/omarchyvr-autod/src/dconfig.rs`. Without an `[auto]` section, auto mode still reads `auto.toml`. |
| `~/.config/omarchyvr/safe-mode` | Flag file. While it exists, nothing runs automatically. |
| `$XDG_RUNTIME_DIR/omarchyvrd/state.json` | The snapshot, rewritten every round (about every 2 s). Treat it as stale when it is older than 10 s. |
| `$XDG_RUNTIME_DIR/omarchyvrd/intent` | Lines: `session=on\|off\|unset`, `by=`, `mode=`, `at=`, `override.<id>=on\|off`. |
| `$XDG_RUNTIME_DIR/omarchyvrd/gpu-gate` | The launcher's format (`boot_id=`, `level=`, `why=`) plus `acked=` and `evidence=`. It is latched per boot. |
| `$XDG_RUNTIME_DIR/omarchyvrd/safe-mode-auto` | The reason omarchyvrd put itself into safe mode. It is removed by `safe-mode off` or `fix omarchyvrd`. |

`$PIMAXD_DIR` replaces `$XDG_RUNTIME_DIR/omarchyvrd`, and `$PIMAX_AUTOD_SOCKET` replaces the socket path (used for tests and
the live check).

## Socket

The socket is `$XDG_RUNTIME_DIR/omarchyvr-autod.sock`, the same one as before. It carries one JSON object per line in each
direction. Every request that auto mode already had still works: `status`, `events`, `pause`, `resume`, `wake`,
`sleep`, `set-mode` and `pair`.

| Request | Reply |
|---|---|
| `{"cmd":"state"}` | The snapshot (below). It has an extra `"autod"` field holding auto mode's `status` object. |
| `{"cmd":"set","session":"on\|off","mode":"desk\|vr","by":"user\|gui\|voice\|cli\|auto","force":false,"voice":true,...}` | `{"ok":true,"intent":"on","toggled":[ids]}` |
| `{"cmd":"toggle","id":"voice","on":false}` | `{"ok":true,"toggled":["voice.listen","voice.captions","voice.act"]}`, or an error for a restart toggle (sixdof, eye, ctrltrack: these apply at the next start and are never applied automatically) |
| `{"cmd":"fix","id":"wayvr"}` | Resets that component's breaker and notices, so the next ladder step runs once. `fix omarchyvrd` leaves automatic safe mode. |
| `{"cmd":"safe-mode","on":true}` | Writes the flag file (or removes it, which also clears automatic safe mode). |
| `{"cmd":"ack-gpu"}` | Acknowledges a caution. needs-reboot cannot be acknowledged. |
| `{"cmd":"wait","for":"overall=ok","timeout_s":120}` | `{"ok":true,"met":true,"state":{...}}`, or `{"ok":false,"met":false,"error":"timeout","state":{...}}` |
| `{"cmd":"events"}` | A stream: auto mode's events as before, plus the reconciler's events (below). |

Errors come back as `{"ok":false,"error":"..."}`.

## Snapshot schema v1 (frozen 2026-10-15)

Fields may be added later; none will be renamed or removed. Times named `t`, `since`, `until` and `at` are seconds on
the daemon's monotonic clock. `unix` is wall time.

```json
{"v":1, "t":812.0, "unix":1792032882.8,
 "overall":"ok|degraded|down|needs-you|needs-reboot",
 "safe_mode":false, "safe_reason":null, "observe_only":true,
 "gpu_gate":"ok|caution|needs-reboot", "gpu_evidence":["kernel: ... Xid ... name=wayvr ..."], "boot_id":"3bfa...",
 "intent":{"session":"on|off|unset","by":"user|auto|cli|adopted|none","mode":"desk|vr","overrides":{"voice.listen":false}},
 "session":{"mode":"desk|vr|null","launcher":"idle|starting|stopping","watchdog":false},
 "components":[
   {"id":"monado","label":"VR runtime (monado-service)","kind":"process","wanted":true,
    "health":"off|starting|ok|degraded|blocked|down|failed|unknown","why":null,"since":12.0,"until":null,
    "pid":8741,"ready":true,"detail":"pid 8741","class":"core","toggle":"restart:monado",
    "restarts":{"used":0,"budget":"2/15m","breaker":"closed|open|half-open"},
    "last_exit":{"class":"expected|collateral|user|crash|unknown","how":"signal 6","t":10.0}}],
 "next_action":{"what":"wait|notify|start|stop|stop-subtree|reset-headset|reapply","id":"usb.cam","why":"waiting 20 s for the camera to return by itself","at":832.0,"level":"L0"},
 "busy":null,
 "autod":{...auto mode status...}}
```

Fields:
- **`components`**: every component of the table in `rust/omarchyvr-components/src/spec.rs`, in table order:
  - devices: `usb.hid`, `usb.cam`, `usb.eye`, `display.dp1`;
  - `gpu.gate`;
  - host settings: `host.caps`, `host.mhs`, `host.runtime-link`;
  - processes: `keepalive`, `broker`, `monado`, `wayvr`, `passthrough`, `ctrltrack`, `perf`, `voice.listen`, `voice.captions`, `voice.act`, `voicehud`, `ai.server`, `floor.follow`, `record` (on demand, [RECORD.md](RECORD.md));
  - features: `eye`, `sixdof`;
  - settings: `eye.laser`, `preset`;
  - job: `anchor.relocalize`.
- **`why`**: blocked components name the dependency they wait for, for example `"why":"usb.cam"`. Other values are text.
- **`budget`**: null for components that omarchyvrd never restarts.
- **`toggle`**: `none`, `live`, or `restart:<id>`.

Event lines on the `events` stream are `{"event":E,"t":T,"id":ID,"text":...}`, where E is one of:
- `log`, `notice`, `critical`;
- `would`: observe-only;
- `step` and `done`: executed;
- `uevent`, `exit`, `kernel`.

`health` events are different: `{"event":"health","t":T,"id":ID,"health":H,"why":W}`.

## What omarchyvrd does once enabled

The plan is the spec; the user decisions of 2026-10-15 apply.

- **Launcher starts (P6).** `set session=on by=cli` with no component toggles marks the session as omarchyvrd's.
  Its components are what the launcher's `start-argv.txt` asks for, applied as per-session overrides:
  - `--6dof`, `--ctrl-track`, `--voice`, `--voice-hud`, `--anchor`, `--no-eye` / `--eye`, `--no-passthrough`,
    `--no-perf` (desk) / `--perf` (vr), `--preset`;
  - an absent opt-in flag means off.

  omarchyvrd.toml stays the default when there is no argv, and for what the launcher has no flag for (ai_server,
  floor_follow, eye_laser). A `set` that carries toggles uses those instead.
- **Adoption.** A Monado that omarchyvrd has not seen before sets the intent to on with `by=adopted`. In an adopted
  session, omarchyvrd restores only what was running. It never adds anything the launcher did not start.
- **Exits** are classified:
  - **expected**: omarchyvrd stopped it.
  - **collateral**: a hard dependency went down within 10 s.
  - **user**: exit 0, SIGINT, SIGTERM, or the launcher's user-stop marker or stop.lock. The component is turned off
    for the session, with a notice.
  - **crash**: a signal, a non-zero exit, a kernel segfault line, a panic or abort in its log, or a systemd-coredump
    record for the pid.
  - **unknown**: an adopted process with no evidence. It is not restarted, and the user is told; Fix restarts it.
  - **Monado** has no stop path other than the launcher (which leaves the marker) or omarchyvrd. Without a marker, its
    exit is a crash.
- **Budgets.** Leaf: 5 per 10 min. XR client: 3 per 10 min. Monado subtree: 2 per 15 min. Device: 1 per 30 min.
  Global: 8 per 10 min, after which omarchyvrd enters safe mode.
  - Backoff: leaf 2, 4, 8, 16, 60 s; XR client 5, 15, 45 s; core 30, 120 s.
  - Only crashes spend budget. Three crashes within 5 min of short runs (under 60 s up) count as flapping.
  - New evidence resets a breaker: a replug, Fix, or a toggle.
- **Monado (desk):**
  1. `omarchyvr desk stop --grace 30 --no-kill` cleans up the leftovers.
  2. `PIMAX_WATCHDOG=1 omarchyvr desk start <start-argv.txt> --no-watchdog --no-notify` restarts the session.
  - In a game (vr mode), omarchyvrd only sends a notice.
  - While the launcher's own watchdog runs, omarchyvrd waits.
  - omarchyvrd never starts anything while the gate says needs-reboot.
  - It never sends SIGKILL. A Monado still alive after `--no-kill`, or in D state for 10 s, latches needs-reboot.
    The user is told the choices (kill it themselves, or reboot), and nothing around Monado is stopped or restarted
    any more; voice on the CPU keeps going.
  - `set session=on force=1` passes needs-reboot once (user decision 9). The plain request is refused with an
    explanation.
- **Leaves (wayvr, passthrough, ctrltrack, perf, voice.\*, voicehud, floor.follow).** omarchyvrd respawns the argv, env
  and cwd it captured from /proc while the process ran. The process is detached with setsid, its log is appended, and
  its pid file is written. The preset is re-applied after a WayVR restart. A WayVR crash together with an Xid 31 is not
  restarted: the user is asked. `ai.server` starts and restarts through `omarchyvr-ai up` (the saved shape: the simple
  model, or Lara + the action model fitted to the free VRAM; `--cpu` if that was its mode) and stops with
  `omarchyvr-ai down`; since 2026-10-16 it is a Work-only session component (rule: Work and
  `AiConfig::ai_servers_wanted()`: ai.toml's `voice_local_models`, or Lara in front of an online brain, `voice_lara`;
  omarchyvrd reads it each round; Play never starts it, a switch to Play stops it; AI.md 16 and 21). The
  keepalive restarts through `systemctl --user restart`.
- **Play has the graphics card (2026-10-17).** The Work-only components (`spec::WORK_ONLY`: `ai.server`,
  `voice.listen`, `voice.captions`, `voice.act`, `voicehud`) are never wanted in a vr (Play) session, whatever an
  override or omarchyvrd.toml says, and one found running there is stopped ("Play: the graphics card is for the
  game"); the voice rules also require desk mode in the spec. A stop or restart of Work by omarchyvrd (its children
  run with `PIMAX_WATCHDOG=1`) keeps the local AI loaded (`desk stop` keeps it in the watchdog; with no Work left,
  `ai.server` is no longer wanted), and a switch to Play still frees it (every `vr start` stops every AI slot first);
  the user's stops (the app's Stop, Exit Work, Switch to Play) stop it at once (docs/PLAY-WORK-AUDIT.md).
- **Unplug:**
  - Desk: after 6 s, a clean ordered stop. The intent stays on, and the session is restored once the HID has been
    present for 4 s and DP is connected.
  - Game: a notice, then omarchyvrd waits. It never stops the game.
- **Camera-only drop** (the HID stays present):
  - omarchyvrd first waits 20 s for the camera to return by itself.
  - If it comes back, the broker and Monado's subtree restart, because SLAM cannot reconnect.
  - If it does not, in desk mode: one automatic `omarchyvr vr reset-headset`, run after the ordered stop, then the
    session is restored. After that, omarchyvrd asks the user.
  - In a game: a notice only.
- **Resume.** No verdicts for 15 s. After that, if Monado is alive but its perf stats are stale (frames frozen), the
  session restarts once in desk mode.
- **Sleep inhibit.** While a session runs and omarchyvrd is not observe-only, omarchyvrd holds
  `systemd-inhibit --what=sleep:idle --mode=block`.
- **Notices** go to `notify-send`, and to a WayVR toast (UDP 127.0.0.1:42069, the same channel omarchyvr-voice uses) while
  WayVR runs. In observe-only mode they are only logged, as `would`.
- **Auto mode.** Its starts and stops also set the intent (`by=auto`). Its stop on unplug is left to omarchyvrd while
  omarchyvrd's core repairs are on. All automatic stops pass `--no-kill`.
