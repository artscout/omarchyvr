#!/bin/bash
# Controller test through OpenXR (windowed Monado, does not use the headset display).
# Press buttons, squeeze trigger/grip, move the sticks while it prints. Usage: ./run_ctrl_test.sh [seconds]
SECS=${1:-30}
B=~/src/monado/build
export XRT_COMPOSITOR_FORCE_XCB=1 XRT_COMPOSITOR_SCALE_PERCENTAGE=10 XR_RUNTIME_JSON=$B/openxr_monado-dev.json
pgrep -x monado-service >/dev/null && { echo "monado-service already running - stop it first"; exit 1; }
(sleep $((SECS + 15)) | $B/src/xrt/targets/service/monado-service >/tmp/pimax-ctrl-svc.log 2>&1 &)
for i in $(seq 40); do [ -S "$XDG_RUNTIME_DIR/monado_comp_ipc" ] && break; sleep 0.25; done
gcc -O2 -o /tmp/ctrlcli "$(dirname "$0")/ctrlcli.c" -lopenxr_loader -lm && /tmp/ctrlcli "$SECS"
pkill -x -INT monado-service
