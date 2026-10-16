# Monado pimax driver (3DoF, windowed compositor) - 2026-10-13

Tree: `~/src/monado`, local branch `pimax` (one commit on top of upstream master e686ba7). Build dir `~/src/monado/build`.

## Build
    sudo pacman -S --needed ninja eigen vulkan-headers vulkan-icd-loader openxr xcb-util-keysyms libxkbcommon-x11 cjson   # (others were present)
    cd ~/src/monado
    cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=RelWithDebInfo -DXRT_BUILD_DRIVER_PIMAX=ON
    ninja -C build        # monado-service, monado-cli, libopenxr_monado.so, openxr_monado-dev.json
libpimax is compiled straight into `drv_pimax` from `PIMAX_LIBPIMAX_DIR` (CMake cache var, default `/home/user/pimax-re/libpimax`: `src/pimax.c`, `src/decode.c`).
`XRT_BUILD_DRIVER_PIMAX` defaults OFF.

## Files added / changed (in the Monado tree)
- `src/xrt/drivers/pimax/pimax_interface.h`, `pimax_prober.c` (VID/PID 34a4:0044 entry, one HMD only), `pimax_hmd.c`
- `src/xrt/drivers/CMakeLists.txt` (drv_pimax), `src/xrt/targets/common/{CMakeLists.txt,target_lists.c,target_builder_legacy.c}`, `CMakeLists.txt`, `cmake/MonadoSetOptions.cmake`.

## Driver behaviour
- libpimax session (wake + keepalive only, nothing else) opened in `pimax_hmd_create`; IMU callback on its thread feeds `m_imu_3dof` (rad/s, m/s^2) and `m_relation_history` (every 4th sample, with world angular velocity for prediction). `get_tracked_pose` = `m_relation_history_get`.
- Device clock (unwrapped u32 us) mapped to monotonic ns with a min-offset tracker.
- Startup: first `PIMAX_BIAS_MS` (2000) samples averaged as gyro bias (window restarts on motion: sum|gyro| > 11 dps or first/second half means differ > 0.3 dps); mean accel seeds the tilt (specific force -> +Y). Pose is identity/not tracked until done. Yaw starts at 0 (no yaw reference).
- Display: one screen 7104x3840 @ 72 Hz, 2 views 3552x3840 (side by side), FOV 98 x 87 deg centred, no distortion (`u_distortion_mesh_set_none`). No lens data / IPD yet.
- Env vars: `PIMAX_LOG` (default info), `PIMAX_AXIS_MODE` (0 identity, 1 = 180 deg about Y: x,z negated), `PIMAX_BIAS_MS`, `PIMAX_PANEL_ROTATION` (0/90/180/270; sets `views[].rot` only, TODO: swap dims/viewport and find the real direction).

## Run
    cd ~/src/monado/build
    export XRT_COMPOSITOR_FORCE_XCB=1 XRT_COMPOSITOR_SCALE_PERCENTAGE=10 PIMAX_LOG=info
    sleep 3600 | src/xrt/targets/service/monado-service        # needs a stdin pipe when not on a tty; stop: pkill -x -INT monado-service
    export XR_RUNTIME_JSON=$PWD/openxr_monado-dev.json
    gcc -o posecli analysis/tools/posecli.c -lopenxr_loader -lm; ./posecli 20                            # tiny headless OpenXR pose printer (source: analysis/tools/posecli.c)
    sleep 60 | /tmp/claude-1000/xrsrc/b/src/tests/hello_xr/hello_xr -g Vulkan2 -ff Hmd -vc Stereo -bm Opaque   # windowed compositor client
`monado-cli probe` lists `Pimax Dream Air`, head role, view count 2.
Scale 10 % keeps the render target small (3552x3840 x 1.4 default would be huge on a window test).

## Test results (headset on desk, keepalived untouched, still active afterwards)
- Bias: 0.303 / 0.715 / 0.444 dps (matches imu-orientation.md), accel (-0.016, 0.986, 0.155) g, 0 restarts.
- Pose over 20 s (VIEW in LOCAL): yaw 0.07 -> -0.02 deg (about -0.3 deg/min while warming), pitch -8.9, roll -0.9 constant; flags 0x7 (orientation valid+tracked). Sane: tilt matches the accel (0.155 g on Z ~ 9 deg).
- hello_xr (Vulkan2) rendered into the Monado window (title "Monado", floating 3552x1920 on workspace 3, fullscreen 0) while posecli ran as a second client.
- Service stopped with SIGINT, keepalived still active.

## Open TODOs
- Axis signs unverified (X right / Z back ~60 %); tilt-test per imu-orientation.md, switch `PIMAX_AXIS_MODE=1` if inverted. Pitch sign with identity: -8.9 deg for a Z-accel of +0.155 g (check against reality).
- Panel 90 deg rotation direction and viewport handling; lens distortion; real FOV/IPD (live IPD in `pimax_imu_sample.ipd_mm` is not used yet).
- Ongoing bias tracking (zero-velocity re-bias), yaw drift correction via SLAM; controllers; direct mode not attempted (hard rule).
- Window size is derived from the screen size (7104x3840 clamped by compositor); window appears on the current Hyprland workspace as a floating window.

## Recovery + re-test (2026-10-13, second session)
- The previous session died mid-commit: `.git/objects/fd/bb6c...` was a 0-byte commit object and `analysis/tools/posecli.c` was empty. Moved the empty object aside, reset `pimax` to upstream e686ba7 (staged tree was intact) and recommitted as 1bd7d6d. `git fsck` still warns about the two reflog entries (harmless).
- posecli.c rewritten (XR_MND_headless + XR_KHR_convert_timespec_time); build: `gcc -o posecli analysis/tools/posecli.c -lopenxr_loader -lm`.
- Re-test, 15 s still on desk: flags 0x7, yaw 0.03 deg, pitch -5.45, roll -0.82, stable (no visible drift in 15 s). keepalived stayed active, service stopped cleanly with SIGINT.
