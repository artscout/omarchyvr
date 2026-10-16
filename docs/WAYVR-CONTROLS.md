# WayVR controls in desk mode (Pimax controllers)

1. **Grip on a screen**: it follows your hand rigidly. Release: it stays there in the room (world-locked, saved on `pimax desk stop`, restored on the next start).
2. **Thumbstick up/down while gripping**: push away / pull closer (0.4-8 m); the size scales with distance so it keeps its apparent size (far = cinema screen).
3. **Grip the same screen with both hands**: pull apart / together to resize (0.3-6 m wide), turn your hands to rotate it. This is the only resize gesture.
4. A short hint ("stick: push/pull . both grips: resize") shows by the grabbing hand. Optional pin (A right / X left click toggles grabbable): `world_lock_pin: true`. Recenter the play space: double-click left menu. **Reset screens** (all back in front): double-click B (right) or `wayvrctl reset-screens`. Unusable screens (bad pose, too close/far, out of view for 10 s) go back by themselves (see wayvr.log "auto-reset").

Config: `pimax desk start` rewrites `~/src/desk-out/wayvr-home-am/config/wayvr/conf.d/30-worldlock.yaml` every start (do not edit; change the constant in `rust/pimax-tools/src/vr/desk.rs`). Static poses live in SLAM space: start the headset at about the same place.
Install (desk stopped, `pgrep -x wayvr` empty): `cp ~/src/target-desk-test/release/wayvr ~/src/target-desk/release/wayvr`, and the launcher: `cd ~/pimax-re/rust && cargo build --release -p pimax-tools && install -m755 target/release/pimax ~/.local/bin/pimax`.

## Controller models and the pointer ray (2026-10-16)
**What you see:** with "Use controllers in Work" on, each controller in use gets a 3D model at your hand, the same
Quest 2 Touch shape SteamVR draws, with the trigger, grip, thumbstick and buttons moving as you press them. It is
hidden while camera hands are the pointer (hands only, the default). App: WayVR page > Next desk start >
"Show 3D controller models (like SteamVR)" (`controller_models`, default on) and "Debug: show pointer and grip axes
on the controllers" (`controller_axes`, default off). Both apply at the next desk start (WayVR keys
`show_controller_models`, `controller_axes`).

**How SteamVR does it:** a driver names a render model (ours: `{oculus}oculus_quest2_controller_left/right`);
SteamVR loads its OBJ parts and moves them from the JSON `motion` blocks (trigger rotate, buttons translate, joystick).
The same JSON has `openxr_grip` and `openxr_aim` components: SteamVR's OpenXR runtime derives the grip and aim poses
from the Touch "raw" pose with them. Apps on Monado can't use those files (Valve's, not redistributable), so WayVR uses
the **WebXR Input Profiles** `oculus-touch-v3` glTF (`@webxr-input-profiles/assets` 1.0.20, **MIT**, Amazon; the
licence excludes Meta's trademarks), embedded from `~/src/wayvr/wayvr/src/res/controllers/oculus-touch-v3/` with its
LICENSE. It is authored in grip space and animated like the WebXR `motion-controllers` library (each part
interpolated between a min and a max node). Placed 2.45 cm forward of the grip so its box matches SteamVR's Quest 2
model in the Touch raw frame (test `the_model_sits_where_steamvr_draws_its_own_and_the_ray_leaves_its_tip`).
Drawn into one stereo projection layer (depth, texture, 0.6 x eye resolution), submitted only while a model shows.
Other overlays (wlx-overlay-s upstream, xrdesktop) draw only rays; OpenXR has `XR_MSFT_controller_model`,
`XR_FB_render_model` and the new `XR_EXT_render_model` for runtime-supplied models, which Monado does not offer.

**Why the ray was "a bit off" (evidence):**
1. The driver's grip was `aim * pitch(45 deg)` at the same point (the LED ring). SteamVR, given that grip, computes
   `raw = grip * inverse(openxr_grip)` and `aim = raw * openxr_aim` = `grip * pitch(-60 deg)` + 9.5 cm: its ray was
   **15 deg lower** and started **~10 cm** from WayVR's (test `the_old_rule_was_fifteen_degrees_off_in_steamvr`), and
   the grip (palm centre, where hands and models are drawn) sat at the ring instead of in the handle.
   Fix (Monado `pimax_ctrl.c`, marker `PIMAX_TOUCH_GRIP`; pimax-hands `touch.rs`, `pimax_hands_tracked_aim_to_grip`):
   optical grip = aim * (Quest 2 `openxr_aim` -> `openxr_grip`) = 60 deg pitch, grip 3.0 cm below and 9.5 cm behind
   the ray origin. SteamVR's derived aim now equals the driver's aim exactly (test
   `steamvr_derived_aim_equals_the_driver_aim`). The aim itself (LED frame pitched 52 deg, `PIMAX_CTRLTRACK_AIM_PITCH_DEG`)
   is unchanged.
2. WayVR smoothed the ray **origin** too (lerp 0.3), so in motion the ray trailed the controller by a few cm. Now only
   the direction is smoothed; the origin stays on the controller tip.
3. Camera hands: the ray went from an estimated shoulder through the index knuckle and ignored where the hand points.
   Now it is turned 35 % towards the hand's own direction (wrist -> knuckle, `HAND_DIR_WEIGHT`), still from the knuckle.

**Check in the headset:** switch on the debug axes, start Work with controllers on: blue (ray direction) at the aim
must leave the front tip of the drawn controller and lie on the ray; the grip axes sit in the handle, blue along the
handle. The drawn controller should cover your real one. If the ray is still pitched for you, tell us (the aim pitch
is a setting, 52 deg).

## Performance readout on the right wrist (`pimax-perf`)
Default in `pimax desk start` (off with `--no-perf`), and `pimax vr start --perf` for games. Four lines on a 10 cm panel on the back of the RIGHT wrist (the watch's pose mirrored, not grabbable, fades when turned away): `HMD 72/72` (compositor fps vs the 72/90 target), `APP 71/72` (main app: the game, or passthrough in desk mode), `miss N /5s` (late or skipped compositor frames in the last 5 s), `cpu x gpu y` (app CPU ms between xrBeginFrame and xrEndFrame, GPU ms until its fence signals). Green from 95 % of the target, yellow under 95 %, red under 85 % (times: green under 80 % of the frame period, yellow under 100 %, red beyond).
Data: Monado (`~/src/monado-eye`, branch desk, `comp_multi_system.c`) writes `$XDG_RUNTIME_DIR/pimax-perf.stats` every 0.5 s when `PIMAX_PERF_FILE` is set (the launcher sets it); `rust/pimax-perf` (`pimax-perf run`) reads it and drives the WayVR custom panel `perf` over `wayvrctl batch`. Needs the Monado relink (setcap) and WayVR `b26591a`+ (`~/src/target-desk/release/wayvr`).
Game mode: the launcher starts a second, minimal WayVR (`--openxr --headless`: an XR_EXTX_overlay session with no screens, no watch, no keyboard; config home `~/src/desk-out/wayvr-home-perf`) plus `pimax-perf`; `pimax vr stop` stops them first. Not yet tried with a real game on the headset.
