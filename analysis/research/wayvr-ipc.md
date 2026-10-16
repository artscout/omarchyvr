# WayVR IPC: what an external program can control

Source: ~/src/wayvr, branch eye-gaze (read-only). Paths relative to that repo.

## 1. Transport, socket, CLI

- Protocol: Unix local socket via `interprocess`, name `"/tmp/wayvr_ipc.sock"` with `GenericNamespaced` (wayvr/src/ipc/ipc_server.rs:540-541; client wayvr-ipc/src/client.rs:129-132). On Linux this is most likely the **abstract** namespace (no file on disk). Verify with `ss -xlp | grep wayvr`.
- Framing: `u32` big-endian length prefix + JSON body (serde_json), `send_packet` ipc_server.rs:34-46; `data_encode`/`data_decode` wayvr-ipc/src/ipc.rs:28-41 (fns at :28 and :37).
- Handshake: `Handshake{protocol_version, magic, client_name}`; magic `"wayvr_ipc"`, `PROTOCOL_VERSION = 4` (wayvr-ipc/src/ipc.rs:25-26; handshake check ipc_server.rs:125-160). Reply `HandshakeSuccess{runtime:"wayvr-server"}`.
- Enums: `PacketClient` wayvr-ipc/src/packet_client.rs:85-101; `PacketServer` wayvr-ipc/src/packet_server.rs:97-106 (files say "keep identical to wayvr").
- Server runs only if the WayVR compositor backend exists: `tick_events` returns early when `wvr_server` is None (wayvr/src/backend/wayvr/mod.rs:317-322). Overlay-level state (OpenXR/OpenVR overlays) is not reachable from the IPC tick (`TickParams`, ipc_server.rs:85-92, has no overlay manager).
- CLI: **`wayvrctl`** (wayvrctl/src/main.rs; `Subcommands` enum :227-313, dispatch `run_once` :96). Has `batch` (reads commands from stdin, :60-95) which is useful for scripting. Also a Rust lib client in wayvr-ipc/src/client.rs (`fn_wvr_*`, `fn_wlx_*` helpers, :324-455).
- Also exists: OSC **send-only** status (wayvr/src/subsystem/osc.rs:17-150, no receive) and UDP `127.0.0.1:42069` for toast notifications only (wayvr/src/subsystem/notifications.rs:95-97). Neither moves overlays.

## 2. Command matrix (PacketClient, packet_client.rs:85-101)

| Need | Status | Existing message / note |
|---|---|---|
| List overlays/windows with poses | **Partial / no pose** | `WvrWindowList(Serial)` -> `WvrWindow{size_x,size_y,visible,handle,process_handle}` (packet_server.rs:33). No pose, no overlay id/name. Handler ipc_server.rs:193-220. `WlxInputState` gives HMD + pointer positions only (ipc_server.rs:167-190; bug: right hand reads `pointers[0]`, line 184). |
| Set pose (position/rotation) of an overlay | **Missing** | Internal: `OverlayTask::Modify(sel, closure)` (backend/task.rs:121-138, closure type task.rs:119). |
| Resize / scale | **Missing** (WayVR window scale also not exposed) | Internal: `OverlayTask::ResizeOverlay(sel,[u32;2])` (task.rs:127, handled manager.rs:220-230) -> `OverlayEventData::ResizeRequest` -> wayvr toplevel `request_size` (overlays/wayvr.rs:794; backend/wayvr/window.rs:90). Only for resizable windows (window.rs:46). Scale: `scale_delta` path in overlays/wayvr.rs:285-305. |
| Show/hide (global) | **Exists** | `WlxShowHide` (ipc_server.rs:437-439 -> :354). |
| Show/hide (single window) | **Exists** (window-level only) | `WvrWindowSetVisible(handle,bool)` (ipc_server.rs:222-233): flips `window.visible` + signal; does not move the overlay. |
| Show/hide (single overlay) | **Missing** | Internal `OverlayTask::ToggleOverlay(sel, ToggleMode)` (manager.rs:232). |
| Switch set | **Exists** | `WlxSwitchSet(Option<usize>)` (ipc_server.rs:440-442 -> :358). |
| Close window | **Missing** | Internal `WayVRServer::close_window` -> `WayVRTask::CloseWindowRequest` (backend/wayvr/mod.rs:683-686; enum :126-135). |
| Close app (cheapest "close") | **Exists** | `WvrProcessTerminate(handle)` (ipc_server.rs:291 -> `process.kill`, backend/wayvr/process.rs:49). Kills the process, not one window. |
| Spawn an app | **Exists** | `WvrProcessLaunch(serial, WvrProcessLaunchParams{name,exec,env,args,icon,resolution,pos_mode,userdata})` (wayvr-ipc/src/packet_client.rs:50; handler ipc_server.rs:236). `pos_mode` = Float/Anchor/Static. |
| Spawn overlay/panel (non-app) | **Missing** | Internal `OverlayTask::Spawn` (manager.rs:399-401, task.rs:135). |
| Query processes | **Exists** | `WvrProcessList`, `WvrProcessGet`. |
| Panel text/color/image/visible | **Exists** | `WlxModifyPanel` (params packet_client.rs:78) -> `WayVRSignal::CustomTask` (ipc_server.rs:366-395; events.rs:60-62). |
| Haptics, input capture, handsfree | **Exists** | `WlxDeviceHaptics`, `WvrInputCapture`, `WlxHandsfree`. |

Shortcut for voice: "move" and "resize" are the real gaps. "Close" can be approximated by `WvrProcessTerminate`; "show/hide one" by `WvrWindowSetVisible` (window only).

## 3. Pose storage

- Runtime struct: `OverlayWindowState` (wlx-common/src/windowing.rs:59-75). `transform: Affine3A` (~:62) is **`skip_serializing`**; `saved_transform: Option<Affine3A>` (:68) is persisted.
- `Positioning` enum (wlx-common/src/windowing.rs:7-21): Floating (default), Anchored, Static, FollowHead{lerp}, FollowHand{hand,lerp}.
- Per-frame: `OverlayWindowConfig::tick` (wayvr/src/windowing/window.rs:139-197) computes `transform` = parent * saved_transform. Parent: HMD (FollowHead), hand pose (FollowHand), `app.anchor` (Anchored), identity (Static) — STAGE space for Static/Floating (:158-166).
- `save_transform` (window.rs:318-328) stores `saved_transform = parent.inverse() * transform`, where parent is snap_upright(HMD) for Floating, HMD for FollowHead, hand for FollowHand, snap_upright(anchor) for Anchored, identity for Static. So **saved poses are head-relative for Floating/FollowHead, anchor-relative for Anchored, stage-absolute for Static**.
- Therefore: setting a pose from outside means writing `saved_transform` via the same parent convention, then `dirty=true` (window.rs:197, :257), or it will be overwritten by the next tick.
- Playspace (world) offset: `playspace.json5` (wayvr/src/state.rs:366-376, `PlayspaceState{openvr_space_center}`).

## 4. Where state is saved

Config root: `$XDG_CONFIG_HOME/wayvr` (fallback `/tmp/wayvr`) — wlx-common/src/config_io.rs:10-18.
- Overlay sets/positions: `~/.config/wayvr/conf.d/zz-saved-state.json5` (wayvr/src/config.rs:295-311; `AutoState{sets,last_set,spawn_overlays,attribs}`). Written by `persist_layout` + `save_state` on OpenXR shutdown (backend/openxr/mod.rs:547-549), OpenVR shutdown (backend/openvr/mod.rs:355-356), and edit-mode exit (windowing/manager.rs:771). `persist_layout` = manager.rs:507. Restored by `restore_layout` manager.rs:606.
- Settings: `zz-saved-config.json5` (config.rs:204-280), saved only when `config_dirty`.
- Playspace: `~/.config/wayvr/playspace.json5` (state.rs:366-376).
- **Not persisted over IPC**: nothing in the protocol writes pose; saves happen only at shutdown or edit-mode exit, so a live pose change survives only if the app later saves.

## 5. Missing commands: where to add them

Common pattern: add a variant to `PacketClient` (wayvr-ipc/src/packet_client.rs:85) **and** a mirror in the wayvr copy (same file is "kept identical"), bump `PROTOCOL_VERSION` (wayvr-ipc/src/ipc.rs:25), add an arm in `process_payload` (ipc_server.rs ~401-450). Overlay-level work needs the overlay manager, which `TickParams` lacks, so route through `WayVRSignal` (wayvr/src/ipc/signal.rs:3-11) and handle in `tick_events` (wayvr/src/ipc/events.rs:33-40, `_overlays` param is already passed). Clients: add a `fn_*` in wayvr-ipc/src/client.rs and a subcommand in wayvrctl/src/main.rs (`Subcommands` :227, `run_once` :96). Roughly 20-30 lines per command plus about 15 lines in wayvrctl.

1. **List overlays with pose** (`WlxListOverlays(Serial)` + `PacketServer::WlxOverlayListResponse`): iterate `OverlayWindowManager::iter` (manager.rs:855), export name, id, `active_state` or `default_state` `saved_transform`/`positioning`/`alpha`. Add to `events.rs` via a response channel, or do it in `tick_events`. ~60-90 lines (packet types + handler + serializable struct).
2. **Set pose** (`WlxOverlaySetPose{overlay, pos:[f32;3], rot:[f32;4]}`): add `WayVRSignal::SetPose`; handle in events.rs:33 by `enqueue(OverlayTask::Modify(OverlaySelector::Name(..), closure))`; closure sets `state.transform` and calls `save_transform` (window.rs:318) plus `dirty=true`. Easiest hook: `OverlayTask::Modify` (task.rs:134). ~40-50 lines.
3. **Resize/scale** (`WlxOverlayResize{overlay, extent:[u32;2]}`): wrap existing `OverlayTask::ResizeOverlay` (task.rs:127; handler manager.rs:220). Only for WayVR windows that are `resizable` (backend/wayvr/window.rs:46). For a pure scale of a panel, use `Modify` on `matrix3` (see overlays/wayvr.rs:296-300). ~25-35 lines.
4. **Show/hide one overlay** (`WlxOverlayToggle{overlay, mode}`): wrap `OverlayTask::ToggleOverlay` (task.rs:125; handler manager.rs:232). ~25 lines.
5. **Close window** (`WvrWindowClose(handle)`): call `wvr_server.close_window` (backend/wayvr/mod.rs:683), which is only reachable from inside `tick_events` (`app.wvr_server` is in `AppState`, state.rs:80). Add `WayVRSignal::CloseWindow` and handle in events.rs. ~25 lines. Alternative today: `WvrProcessTerminate` (kills whole app).
6. **Spawn overlay/panel** (`WlxOverlaySpawn{name}`): wrap `OverlayTask::Spawn` (manager.rs:399) which needs a `CreateOverlayTask` closure; practical only for known built-ins. Apps: use `WvrProcessLaunch` (exists). ~50 lines, low priority.

## 6. Bugs noticed

- `handle_wlx_input_state` reads `pointers[0]` for both hands (ipc_server.rs:178-186).
- `WvrWindow` lacks overlay id, name and pose, so a window cannot be mapped to an overlay from IPC (window_to_overlay exists only inside the compositor, backend/wayvr/mod.rs:145-146).
