# WayVR: eye-gaze pointer and hands

Source: `~/src/wayvr`, branch `eye-gaze` (commits a8a2469 gaze, 88db4e0 hands). Release build: `CARGO_TARGET_DIR=~/src/target-desk cargo build --release`.

## Gaze pointer
- Pose: the existing "handsfree" action space bound to `/user/eyes_ext/input/gaze_ext/pose` (profile `ext/eye_gaze_interaction`), located each frame at the predicted display time in stage space.
- Filter: One Euro on the gaze direction (`gaze_smoothing` 0..1 -> min cutoff 8 Hz..0.3 Hz, beta 2.5), then a fixation dead zone (`gaze_deadzone_deg`, the cursor trails the target by that angle). Invalid / NaN / frozen (45 identical frames) / no sample for 500 ms -> gaze off, nothing happens.
- Pointing: gaze borrows a controller slot whose ray hits nothing (and is not grabbing), so a controller ray on an overlay always wins. Gaze only hovers. A controller trigger (either hand) that is not aimed at an overlay clicks where you look. Optional dwell click (`gaze_dwell_ms` > 0, 90 ms press, must look away before the next one). Cursor: small plus drawn on the overlay (turns red while clicking).
- Disabled while the old `handsfree_pointer: EyeTracking*` modes are in use (they already did eye aiming + pinch).
- Config keys (conf.d yaml): `gaze_pointer` (false), `gaze_dwell_ms` (0), `gaze_smoothing` (0.5), `gaze_deadzone_deg` (0.6).

## Hands
- `XR_EXT_hand_tracking` enabled if offered; one tracker per hand; joints located in stage space at predicted display time. Drawn with WayVR's line pool (flat quad layers, alpha 0.45): per hand 5 finger strokes (proximal->tip) + 2 palm strokes = 14 extra quad layers. No spheres/capsules (the renderer only has flat quads). Hand hidden when inactive or a needed joint position is invalid.
- `show_hands` (true); no effect without hand tracking.
- Pinch click: `hand_pinch_click` (true, needs restart) adds left/right bindings on `ext/hand_interaction_ext` (aim_ext pose, pinch_ext/value = click) to `openxr_actions.json5`. Which profile wins when controllers and hands are both active is up to the runtime (untested).

## Test (user)
Config file `~/src/desk-out/wayvr-home-am/config/wayvr/conf.d/20-gaze.yaml` (next to `10-am.yaml`, which sets `capture_method: screencopy`).
```
PIMAX_EYE=1 XR_RUNTIME_JSON=~/src/monado-eye/build/openxr_monado-dev.json IPC_IGNORE_VERSION=1 \
  ~/src/target-desk/release/wayvr --openxr --show --no-autostart
```
(use the same XDG_* home env as the overnight recipe; start the Monado service with `PIMAX_EYE=1` as in eye-monado.md). Look at an overlay: a small cursor follows the gaze; pull a trigger while looking (not pointing a controller at an overlay) to click. Try `gaze_dwell_ms: 800`. Log lines: "Hand tracking enabled", "Missing EXT_..." warnings.
Not run offline: no headset/runtime tests. Note: creating the gaze space (done at startup by stock WayVR already) makes Monado open the EyeChip.

## Live test 1 (14 Oct): no gaze, no hands - offline analysis
- Overlay focus is NOT the cause: WayVR logged SYNCHRONIZED -> VISIBLE -> FOCUSED within 10 ms of start. Monado's `ipc_server_process.c` (handle_focused_client_events) sets every overlay session visible+focused ("Set all overlays to always active and focused"), so a lone overlay gets FOCUSED and xrSyncActions works. Controllers work as overlay for the same reason. `Session is visible but not active` is a harmless start-up race in the multi compositor (frame delivered before session_active), once. Client 2 in the Monado log is `libmonado` (same pid 104365, WayVR's own device-list connection), not an app.
- Hand interaction bindings: invalid paths. Per Monado `bindings.json` / the spec the profile has `/input/aim/pose` and `/input/grip/pose` (pinch_ext/value etc. are right); `aim_ext` does not exist. Fixed in `openxr_actions.json5`. Note: Monado only routes `ext/hand_interaction_ext` if some xdev provides XRT_INPUT_HAND_PINCH_VALUE (device type XRT_DEVICE_EXT_HAND_INTERACTION; only the SteamVR driver shim does). The Pimax driver has no such device, so pinch-click will stay silent here even with the right paths (would need pimax_ctrl to expose it, e.g. pinch = trigger).
- Hands: the hand trackers come from the controllers (`pimax_ctl_get_hand_tracking`): `is_active` is true only if that controller sent a report in the last 1.5 s (`ACTIVE_TIMEOUT_NS`). xrLocateHandJointsEXT then returns isActive=false and WayVR hides the hand. Without awake controllers there are no hands at all. When active, the wrist sits at the synthesized grip pose: 0.20 m to the side, 0.30 m below, 0.35 m in front of the head (yaw only), i.e. low in the field of view (about 40 deg below the horizon); look down to see them. Joint flags are valid (position valid, not tracked), so WayVR draws them. The lines use the same pool as controller rays, so the missing `XR_FB_composition_layer_alpha_blend` is not the reason.
- Gaze, most likely cause: WayVR borrowed a controller slot only if its ray hits nothing. Pimax controller poses are synthesized (fixed offset from the head, pointing forward, always active), so both rays hit the panel you look at: no free slot, gaze unusable. New config `gaze_over_controllers` (set to true in 20-gaze.yaml): while gaze is valid the controller rays do not hover (their hover is released), the trigger clicks where you look; a controller that holds a grab keeps it.
- Other gaze checks: eye profile binding had no error; HMD input[1] is active (U_DEVICE_ALLOCATE sets all inputs active); gaze relation returns orientation-valid only (HMD has no position tracking), WayVR substitutes the HMD position.
- New diagnostics for the next run:
  - Monado, every 5 s while a client queries gaze: `Pimax eye: state 2, N gaze queries in 5.0 s: V valid, I invalid (eyes lost), S stale, X no sample; chip samples R/s` (state 2 = streaming).
  - Monado: `hand tracking left|right: active|inactive (last report T s ago)` on every change.
  - WayVR every 5 s: `gaze: F frames, located(orientation valid) L, accepted A, location flags 0x.., active .., slot ..` and `hand left|right: F frames, runtime-active N, joints valid (drawn) M`.
- Reading it: gaze located 0 -> Monado side (action inactive or no valid sample, see the Monado line); located high but `active false`/no cursor -> WayVR gating; hands runtime-active 0 -> controllers silent (wake them with a button press) .
