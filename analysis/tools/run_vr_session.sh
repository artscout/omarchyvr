#!/bin/bash
# Long-running Monado (Wayland direct mode, Pimax Dream Air) for OpenVR games via xrizer. Ctrl-C to stop.
# Usage: run_vr_session.sh [logdir]     (env: SCALE=100, XRT_COMPOSITOR_DESIRED_MODE=2)
OUT=${1:-/tmp/pimax-vr-session}; mkdir -p "$OUT"
M=$HOME/src/monado/build
if pgrep -x monado-service >/dev/null; then echo "monado-service already running; stop it first"; exit 1; fi
export XRT_COMPOSITOR_FORCE_WAYLAND_DIRECT=1 XRT_COMPOSITOR_WAYLAND_CONNECTOR=DP-1
export XRT_COMPOSITOR_FORCE_NVIDIA=0 XRT_COMPOSITOR_DESIRED_MODE=${XRT_COMPOSITOR_DESIRED_MODE:-2}
export XRT_COMPOSITOR_LOG=${XRT_COMPOSITOR_LOG:-info} XRT_COMPOSITOR_SCALE_PERCENTAGE=${SCALE:-100} PIMAX_LOG=${PIMAX_LOG:-info}
export XR_RUNTIME_JSON=$M/openxr_monado-dev.json
export IPC_IGNORE_VERSION=1   # client lib / service git tags differ in this dev build

cat <<TXT
*** Keep the headset still for 3 s (gyro calibration) ***
Monado log: $OUT/monado.log        Ctrl-C to stop.

Steam launch options (Properties > General > Launch Options), Proton / native games:
  PRESSURE_VESSEL_FILESYSTEMS_RW=\$XDG_RUNTIME_DIR/monado_comp_ipc XR_RUNTIME_JSON=$M/openxr_monado-dev.json %command%
Needed because Proton runs in the pressure-vessel container: the Monado IPC socket
(\$XDG_RUNTIME_DIR/monado_comp_ipc) and the Monado build under \$HOME must be visible.
If the game ignores ~/.config/openvr/openvrpaths.vrpath add:
  VR_OVERRIDE=$HOME/src/xrizer/target/release
xrizer log: ~/.local/state/xrizer/xrizer.txt  (RUST_LOG=debug for more)
Run SteamVR-free: do not start SteamVR itself; Steam may still show "SteamVR not running", ignore it.
TXT
# monado-service needs a pollable stdin: feed it from a long sleep
trap 'pkill -x -INT monado-service; exit 0' INT TERM
sleep 2147483647 | "$M/src/xrt/targets/service/monado-service" >"$OUT/monado.log" 2>&1 &
wait
