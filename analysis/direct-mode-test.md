# Monado direct mode smoke test (2026-10-13)

Result: Wayland DRM lease from Hyprland works on NVIDIA 610.57.04; Monado direct mode presented frames at 72 Hz;
headset firmware logged `fps calc 72` and `Panel ON` (~6.7 s after start). Visual check by the user pending.

## Prerequisites (read-only)
- `wayland-info` (pacman wayland-utils): Hyprland advertises `wp_drm_lease_device_v1` (path /dev/dri/card1) with
  one connector: id 826, DP-1, "Pimax Tech. CO. LTD Pimax 0x00000011 (DP-1)".
- `vulkaninfo` (pacman vulkan-tools): NVIDIA 4090 exposes VK_EXT_acquire_drm_display, VK_KHR_display,
  VK_EXT_acquire_xlib_display, VK_KHR_wayland_surface.

## Monado settings (names confirmed in comp_settings.c / comp_window_direct_wayland.c)
- XRT_COMPOSITOR_FORCE_WAYLAND_DIRECT=1, XRT_COMPOSITOR_WAYLAND_CONNECTOR=DP-1, XRT_COMPOSITOR_PRINT_MODES=1,
  XRT_COMPOSITOR_DESIRED_MODE=<index>, XRT_COMPOSITOR_FORCE_NVIDIA=0, XRT_COMPOSITOR_SCALE_PERCENTAGE (default 140).
- IMPORTANT: auto mode choice picks 7104x3840@90 (highest refresh). The firmware was told 72 Hz, so set
  `XRT_COMPOSITOR_DESIRED_MODE=2` (index of 7104x3840@72). Mode list: 0 3840x2160@72, 1 1920x1080@72,
  2 7104x3840@72, 3 7104x3840@90 (check with PRINT_MODES, order comes from the Vulkan display).
- Run 1 (auto, 90 Hz): lease and frames fine, but firmware: `fps calc 90 ... required fps 72`, `wait_video_info_stable`
  error loop, no Panel ON. Run 2 (DESIRED_MODE=2): Panel ON.

## Driver changes (~/src/monado, branch pimax, commit de3a36e)
pimax_hmd_create() calls pimax_video_prepare(72) after connect and writes `72` to $XDG_RUNTIME_DIR/pimax-rate
(log: `pimax_video_prepare(72) -> 0`, `wrote 72 to /run/user/1000/pimax-rate`). Display was already 7104x3840@72
with two 3552x3840 views side by side; PIMAX_PANEL_ROTATION kept.

## Test client
analysis/tools/xr_eyetest.c (Vulkan, XR_KHR_vulkan_enable2): L eye blue, R eye red, white arrow pointing UP, black
letter L/R at the bottom, yellow square at each eye image top-left, white border. Build:
`gcc -O2 -o analysis/tools/xr_eyetest analysis/tools/xr_eyetest.c -lopenxr_loader -lvulkan -ldl`
(binary used in the test was built into the Claude scratchpad; rebuild with the command above).
Runs XR_EYETEST_SECONDS (default 20) then exits the session.

## Commands
```
cd ~/pimax-re/analysis/tools
gcc -O2 -o xr_eyetest xr_eyetest.c -lopenxr_loader -lvulkan -ldl
XRT_COMPOSITOR_DESIRED_MODE=2 SECS=25 ./run_direct_test.sh /tmp/pimax-direct-test
# logs: monado.log client.log fwlog.txt in that dir. Manual stop: pkill -x -INT monado-service
```
(run_direct_test.sh sets the env above, starts monado-service via `sleep 40 | monado-service`, passive
pimax_logtail.py 30, the client, then SIGINT to monado-service.) Panels light ~6 s after the stream starts.
Per-eye rotation test: add `PIMAX_PANEL_ROTATION=90` (or 270, 180) to the environment of the script.

## Log excerpts (run 2)
```
pimax_video_prepare(72) -> 0 / wrote 72 to /run/user/1000/pimax-rate
comp_window_direct_wayland_init Requesting connector DP-1
_drm_lease_device_drm_fd Available DRM lease device: /dev/dri/card1
_lease_connector_done connector DP-1 (...) id: 826
_lease_fd Lease granted
compositor_try_window Target backend wayland-direct initialized!
get_primary_display_mode found display mode 7104x3840@72.00
client: 1801 frames in 25 s (~72 fps), session states ready/synchronized/visible/focused/stopping
firmware: fps calc 72 cur 72 ... required fps 72 b_vide 1
  6.10 state DISPLAY_STATE_WAIT_VIDEO_STABLE => ..._WAIT_PANEL_WORK_WELL
  6.13 state => DISPLAY_STATE_WORK_ON ; hmd: request_panel_on 1 ; 6.70 Panel ON
```

## Next steps
- User to look in the headset during a run: check colours (L blue left eye, R red right eye), arrow direction and
  "L"/"R" orientation; then pick PIMAX_PANEL_ROTATION (0/90/180/270) and re-check. With 90/270 the per-eye
  panel dims likely need to be 3840x3552 in the driver (still TODO in pimax_hmd.c).
- Without distortion the image is un-warped; FOV values are guesses.
- Make DESIRED_MODE selection automatic (driver or comp patch preferring the HMD's nominal 72 Hz mode).

## Windowed check of the per-eye layout (2026-10-13, no headset)
Windowed XCB (`XRT_COMPOSITOR_FORCE_XCB=1`, new debug env `PIMAX_DEBUG_DIV=2` so the window fits a 1920x1080 monitor) + xr_eyetest,
captured with `grim -g` from the `hyprctl clients -j` geometry. Screenshots in analysis/frames/monado/.
- Before (rot 0): left half = blue + arrow up + L, right half = red + arrow up + R (window_before_rot0.png). The
  compositor output is already correct (also at SCALE=100): left view -> left half, right view -> right half. The
  "blue/L in both eyes" seen in direct mode is therefore NOT produced by Monado/driver/client; the 7104x3840 frame
  is right, so the duplication happens downstream (headset bridge/firmware or panel routing).
- After: driver gets `PIMAX_ROT_LEFT` (default 90 = pre-rotate clockwise), `PIMAX_ROT_RIGHT` (default 270 = ccw),
  `PIMAX_MIRROR_LEFT/RIGHT`. Window: left half arrow points RIGHT (yellow marker top-right), right half arrow points
  LEFT (marker bottom-left) (window_after_rot_L90_R270.png). Each eye display is 3840x3552, viewport 3552x3840.

## MILESTONE 2026-10-13: correct stereo in the headset
After the NVIDIA 2Heads1OR patch: Monado direct mode + pimax driver (PIMAX_ROT_LEFT=90, PIMAX_ROT_RIGHT=270 defaults),
XRT_COMPOSITOR_DESIRED_MODE=2 (7104x3840@72): user sees LEFT eye blue/L arrow UP, RIGHT eye red/R arrow UP.

## MILESTONE 2026-10-13: head-tracked 3D VR in the headset
analysis/tools/run_xr_room.sh (xr_room, world-locked sky/markers) with the pimax driver (3DoF IMU, lens distortion from
~/.config/pimax/lens.conf defaults): user reports everything in place, smooth, distortion "similar to Windows";
slight reflections at the edges (likely lens glare / edge of distortion mesh; tune later).
