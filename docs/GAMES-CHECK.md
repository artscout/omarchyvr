# VR games check (`omarchyvr vr game-check`)

"Games show nothing in VR": one check that finds the usual causes (LESSONS rows 20, 23, 27). App: Ready check >
**Check VR games** (the card shows PASS/WARN/FAIL per line, the one fix, and a Copy button for the launch options line).
CLI (for agents and tests): `omarchyvr vr game-check [--appid N] [--json] [--no-container]`, exit 1 on any FAIL.

| Line | What it checks | Typical fix |
|---|---|---|
| service | `monado-service` runs and `$XDG_RUNTIME_DIR/monado_comp_ipc` is a socket | Start VR |
| openxr | `~/.config/openxr/1/active_runtime.json` -> a library inside the running Monado build | Start VR (sets it) |
| openvr | first entry of `~/.config/openvr/openvrpaths.vrpath` is xrizer; FAIL names SteamVR as the one who changed it (it rewrites the file on every start; row 20) | "Use xrizer + Monado" |
| container | a tiny OpenXR client (`xrCreateInstance` + `xrGetSystem`, no session) runs INSIDE Steam's runtime (`SteamLinuxRuntime_4` = Proton Experimental, and `sniper`) without and with `PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1`; WARN = needs the variable (row 27) | variable in Steam's env or in Launch Options |
| steam-env | the variable is set in the running Steam's `/proc/PID/environ`, or in the app's `LaunchOptions` (`userdata/*/config/localconfig.vdf`, read only) | paste `PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1 %command%` |
| last-run | the game's `Player.log` (Proton prefix) and `~/.local/state/xrizer/xrizer.txt`: `InterfaceNotFound` (OpenVR was not xrizer), `RUNTIME_UNAVAILABLE` (container did not reach Monado), session `FOCUSED` | fix the FAIL lines, run the game again |

Work mode (2026-10-15): with "Games run with: SteamVR" saved, Work (`omarchyvr desk start`, Monado + WayVR) no longer
points `active_runtime.json` at Monado: the global OpenXR and OpenVR runtimes keep the user's games choice, and the
session's own OpenXR apps (WayVR, passthrough, the HUDs, room setup) get `XR_RUNTIME_JSON` for Monado directly. Games
are played from Play (SteamVR), not inside Work. While Work runs and SteamVR does not, the service line is a WARN
("Work is running ... games are played from Play"; fix: stop Work and press Play, or choose Games run with: Monado),
the openxr/openvr lines are judged against SteamVR, and the container test is skipped.

Notes
- The test client is C (`PROBE_C` in `rust/omarchyvr-tools/src/vr/game_check.rs`), built once with `cc` into
  `~/.local/share/omarchyvr/bin/xr-probe`. It `dlopen`s the loader, so it runs in any container. Runtimes without an OpenXR
  loader (sniper) are reported SKIP. The OpenVR-through-xrizer container test is not run (reported SKIP); the last-run
  line covers it.
- The check only reads files and starts a short client; a running VR session sees one extra short-lived client.
- `--appid` defaults to the most recently played app in `localconfig.vdf`. The last run's failure is shown as WARN when
  all settings above pass (it is then from before the fix: start the game again).
- Tests: `cargo test -p omarchyvr-tools game_check` (fixtures: vrpath, localconfig.vdf, Player.log, environ);
  `cargo test -p omarchyvr-tools game_check -- --ignored` runs the real container test (needs Steam and a running Monado).
