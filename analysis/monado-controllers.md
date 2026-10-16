# Monado pimax driver: controllers (2026-10-13)

Files (Monado tree, branch `omarchyvr`): `src/xrt/drivers/pimax/pimax_ctrl.{c,h}`; hooks in `pimax_hmd.c` (callbacks on_controller, on_controller_status, on_state), `pimax_prober.c` (returns HMD + 2 controllers), `drivers/CMakeLists.txt`.
Client: `analysis/tools/ctrlcli.c` (`gcc -o ctrlcli analysis/tools/ctrlcli.c -lopenxr_loader -lm`, `./ctrlcli 20`; headless, touch profile, prints both hands 5x/s incl. aim pose).

## Design
- One libpimax session (the HMD's). The HMD callbacks forward controller reports/0x37 status/connect state to the controller context.
- Attach thread (pimax_controller_attach sleeps, so not on the session thread): burst for slot 2 and 3 at start and on every (re)connect; re-sent when a 0x37 heartbeat is newer than the last attach and the last report and no report for >1.5 s (min 2.5 s between bursts). Log lines: `controller slot N attach burst #k (reason)`.
- Devices: `XRT_DEVICE_TOUCH_CONTROLLER`, left/right hand, inputs: X/A click+touch, Y/B click+touch, menu (left MENU_CLICK; right SYSTEM_CLICK, unverified), squeeze value (grip byte/255), trigger value + touch (derived, > 6 %), thumbstick, click, touch, grip+aim pose. Inputs go inactive 1.5 s after the last report.
  Report A bit -> X (left) / A (right), B bit -> Y / B.
- binding_profiles: `khr/simple_controller` (select = trigger, menu = left menu / right B, poses) and Valve Index (trigger, squeeze, thumbstick, A/B, poses). Haptics: set_output is a no-op.
- Battery: get_battery_status from byte 46 (bit7 charging).
- The controllers share the HMD's tracking origin; otherwise the builder adds its default 3DoF hand offset (+-0.2, 1.3, -0.5) on top.

## Pose (PIMAX_CTRL_POSE)
- `fixed` (default): +-0.20 m side, 0.30 m below, 0.35 m in front of the head, following head yaw only, orientation = head yaw. Verified: located in LOCAL at (-0.20,-0.30,-0.35)/(0.20,-0.30,-0.35).
- `imu`: same position; orientation from m_imu_3dof (accel raw/1024 g, gyro raw * (pi/180)/PIMAX_CTRL_GYRO_LSB_DPS, default 16.4 = GUESS, axes assumed = OpenXR axes). Startup: ~1 s (250 samples) still window for gyro bias + tilt seed; yaw reference = head yaw at that moment, no yaw correction (drifts). Until the bias is done the orientation is the head yaw (same as fixed). Live: |acc| = 0.97 / 1.00 g at rest, which confirms 1024 LSB/g. Gyro scale and axis signs are not verified.

## Test (headset on the desk, no display, controllers awake but untouched)
Windowed Monado (XRT_COMPOSITOR_FORCE_XCB=1, scale 10 %) + ctrlcli: both slots attached on the first burst (`first report (slot 3)`, `(slot 2)`), profile `/interaction_profiles/oculus/touch_controller` for both hands, all values 0 (nobody pressed anything), act=1, pose flags 0x7. imu mode: bias done on both, orientation stable. SIGINT/stdin close shuts down cleanly. Button/stick values through OpenXR NOT yet seen with a real press.

## Test with controllers in hand
    cd ~/src/monado/build; export XRT_COMPOSITOR_FORCE_XCB=1 XRT_COMPOSITOR_SCALE_PERCENTAGE=10 XR_RUNTIME_JSON=$PWD/openxr_monado-dev.json
    (sleep 60 | src/xrt/targets/service/monado-service > /tmp/svc.log 2>&1 &) ; sleep 10
    /tmp/claude-1000/ctrlcli 30      # press every button, squeeze trigger/grip, move the stick; PIMAX_CTRL_POSE=imu on the service for IMU orientation

## Live verification through OpenXR (2026-10-13, user pressing everything, run_ctrl_test.sh)
Both hands on /interaction_profiles/oculus/touch_controller: X, Y, A, B, left menu, triggers 0..1.00, grips 0..1.00,
thumbsticks (left x -1.00 at full left, right x +1.00 at full right, up positive), stick clicks all CORRECT.
Right menu/system not exercised. Pose = fixed (no optical tracking). Exit error XR_ERROR_SESSION_NOT_STOPPING is ctrlcli
calling xrEndSession without xrRequestExitSession (test-client bug, harmless).

## Hand tracking from the controllers (2026-10-14, branch `hands`, details in `analysis/overnight/hands.md`)
The controllers also expose `XRT_INPUT_HT_CONFORMING_LEFT/RIGHT` (OpenXR: `XR_EXT_hand_tracking`, data source `CONTROLLER`). Curl logic is the Rust
crate `rust/pimax-hands` (trigger -> index, grip -> middle/ring/little, touch/click bits -> thumb on A/X, B/Y, stick or menu; One Euro filters,
thumb state machine); `pimax_ctrl.c` only forwards the reports and runs `u_hand_sim_simulate_generic` with the wrist at `grip pose * offset`.
`PIMAX_CTRL_HANDS=0` switches it off, `PIMAX_CTRL_HANDS_DEMO=1` replaces the reports by a scripted pattern (tests). Test: `analysis/tools/run_hand_test.sh`.
Finding from the button captures: while the trigger or grip is squeezed the capacitive touch bits (B, stick) blink for 50-100 ms with no thumb on them,
and the stick bit flickers during stick sweeps; the mapping ignores touch bits younger than 120 ms (clicks and stick deflection count at once) and bridges gaps up to 150 ms.
