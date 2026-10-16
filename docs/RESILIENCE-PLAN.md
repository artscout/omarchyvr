# Resilience plan (short version)

The full plan, with tables and scenarios, is [../analysis/resilience-plan.html](../analysis/resilience-plan.html). This is a plan only (2026-10-15); no code has changed.

## Review: what we have and what is missing
Four actors watch the session, and none of them sees the whole picture:
- the launcher watchdog (`omarchyvr-tools/src/vr/supervise.rs`);
- auto mode (`omarchyvr-autod`, not installed);
- the GUI monitor and preflight (`omarchyvr-gui-core`);
- the `pimax-device` checks.

The checks exist four times: launcher preflight, GUI preflight, GUI guards and pimax-device. The GPU-line classifier exists twice with different policies, and USB presence is read three ways.

**Conflict:** autod stops a session after a 6 s unplug, and the watchdog restarts it after 6 s of the same unplug.

What happens when a device disappears (from code and logs):
- **libpimax:** reconnects the HID by itself (`libpimax/src/pimax.c:292-358`).
- **Monado driver:** has no state callback, so the pose freezes silently.
- **Direct mode:** never re-acquired after a DP drop.
- **Broker:** exits on the first V4L2 error and never reopens (`pimax-cam/src/broker.rs:655`, `v4l2.rs:569`).
- **Broker clients:** passthrough, ctrltrack and roommap exit and never reconnect.

**Gaps:**
- **G1:** No GPU safety gate. The watchdog's stop can SIGKILL a stuck Monado, which wedges the display until a reboot (`analysis/nvidia-flip-hang.md` §0).
- **G2:** The watchdog uses up its 3 tries while the cable is out.
- **G3:** WayVR, passthrough, voice and perf crashes go unnoticed.
- **G4:** Any camera blip costs a full session restart.
- **G5:** Monado is blind to HID loss.
- **G6:** Nobody owns user intent.
- **G7:** No suspend/resume handling.
- **G8:** A `cap_sys_nice` drop is only seen at preflight.
- **G9:** WayVR may still be tearing down when Monado gets SIGINT (Xid 31).
- **G10:** WayVR's 10 s screen reset is not visible.

## Decisions
1. **One owner: `omarchyvrd`**, grown out of `omarchyvr-autod`.
   - Why: it is already a std-thread user daemon with a pure state machine, a simulator, a JSON-lines socket, 2 MB RSS and only libc as a dependency.
   - systemd supervises the daemon. The CLI, GUI and voice are clients.
   - It adopts running sessions from pid files and pidfds, so restarting the daemon never stops a session.
2. **A shared `omarchyvr-components` crate** with a `Component` trait: `id, kind, deps, wanted, toggle, probe, start, stop, set_live, ladder, grace`.
   - One module per component.
   - Pure graph, policy and `reconcile()` functions, tested with the autod-style simulator.
3. **Desired state:**
   - `~/.config/omarchyvr/omarchyvrd.toml` holds the persistent toggles and budgets.
   - `$XDG_RUNTIME_DIR/omarchyvrd/intent` holds whether a session should run (on/off), plus who set it.
   - `~/.config/omarchyvr/safe-mode` is a flag file.
4. **Actual state:** `{"cmd":"state"}` on the socket, mirrored to `$XDG_RUNTIME_DIR/omarchyvrd/state.json`.
   - Per-component health: off, starting, ok, degraded, blocked, down, failed, unknown.
   - An overall value: ok, degraded, down, needs-you, needs-reboot.
5. **Events, without root or new dependencies:**
   - a netlink kobject-uevent socket, used only as a hint followed by a sysfs re-scan;
   - pidfd for process exits;
   - a `journalctl -k -f` child for kernel lines;
   - a tail of the Monado log;
   - probe timers;
   - a jump between BOOTTIME and MONOTONIC to detect resume.
6. **Careful restarts:**
   - Per-class budgets: leaf 5/10 min, XR client 3/10 min, Monado subtree 2/15 min, device 1/30 min, global 8/10 min. Each class has backoff and a circuit breaker.
   - Only crashes spend budget. Collateral exits, user exits and waits for an absent device are free.
   - Flapping counts as failing.
   - A breaker resets on new evidence: a replug, or a Fix from the user.
   - Restarts follow dependency order. A stop waits until WayVR has actually exited before it sends SIGINT to Monado.
   - Grace periods: camera 150 s after a reset and 20 s to return by itself; Monado 45 s to "gyro bias done"; USB 4 s to plug and 6 s to unplug; resume 15 s.
7. **The user's intent wins.** A manual stop sets the intent to off, and a process the user closed is turned off. A toggle that needs a restart is never applied automatically.
8. **GPU safety gate**, latched per `boot_id`:
   - **caution:** WayVR Xid 31.
   - **needs-reboot:** any other Xid, Idling, a stuck layer, an acquire failure, a stuck flip, or Monado hung or killed.
   - The daemon never SIGKILLs Monado on its own. It never touches the DRM device or hotplug, and it never sends HID bytes itself.
9. **Safe mode:** probes and the snapshot keep running, but nothing runs automatically. It turns on by itself after a restart storm.

## Phases (lanes do not share files)
| Phase | Content | Agent |
|---|---|---|
| P0 | Watchdog hotfixes: no automatic SIGKILL, wait for the device without spending tries, no restart after Xid or acquire failure, stop waits for WayVR to exit | Sonnet |
| P1 | `omarchyvr-components` core: model, graph, policy, read-only probes, `pimax health` | Sonnet |
| P2 | Events: uevent, pidfd, kernel follower, resume; `pimax health --watch` | Sonnet |
| P3 | Move start and stop into components (byte-identical dry-run golden files) | Opus |
| P4 | omarchyvrd observe-only (safe mode), toml and intent, snapshot and socket, sim scripts per scenario | Opus |
| P5 | Leaf remediation (voice, perf, ctrltrack, ai, passthrough, wayvr) | Sonnet |
| P6 | Launcher as a client (`set` + wait, `--no-daemon` fallback), watchdog retired | Sonnet |
| P7 | Core remediation: Monado subtree, gate, unplug and replug, hub drop, resume, sleep inhibit | Opus |
| P8 | GUI System page (list, dots, toggles, events, Fix, safe mode, reboot banner) | Sonnet |
| P9 | Self-healing inside components: broker reopen and stall, clients reconnect, ctrltrack heartbeat, Monado `on_state`, WayVR teardown | Sonnet / Opus |
| P10 | Docs, unit enablement | Haiku |

The work runs in four lanes:
- **Lane A (omarchyvr-tools):** P0 → P3 → P6.
- **Lane B (omarchyvr-components and autod):** P1 (with P2 alongside) → P4 → P5 → P7.
- **Lane C (pimax-cam, passthrough, ctrltrack, roommap, Monado, WayVR):** P9, any time.
- **Lane D (GUI):** P8, after the P4 schema.

P0, P1, P2 and P9 can start now.

## Open questions
1. Should omarchyvrd start at login?
2. Are the budgets right?
3. Should a WayVR Xid 31 at shutdown block manual starts, or only automatic ones?
4. If the headset is unplugged mid-game, should we only notify, stop after N minutes, or restart on replug?
5. Should a camera-only drop in desk mode trigger an automatic reset-headset?
6. Should we block sleep during a session, or stop the session before suspend?
7. Should `auto.toml` merge into `omarchyvrd.toml`?
8. Should there also be in-headset toasts?
9. Should `--force` be allowed past needs-reboot?
10. When may P9 touch the Monado and WayVR branches?
