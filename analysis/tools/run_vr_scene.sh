#!/bin/bash
# Head-tracked VR scene via Monado direct mode. Usage: [SECS=60] run_vr_scene.sh [outdir]
OUT=${1:-/tmp/pimax-vr-scene}; mkdir -p "$OUT"
SECS=${SECS:-60}
M=$HOME/src/monado/build
export XRT_COMPOSITOR_FORCE_WAYLAND_DIRECT=1 XRT_COMPOSITOR_WAYLAND_CONNECTOR=DP-1 XRT_COMPOSITOR_PRINT_MODES=1
export XRT_COMPOSITOR_FORCE_NVIDIA=0 XRT_COMPOSITOR_DESIRED_MODE=${XRT_COMPOSITOR_DESIRED_MODE:-2}
export XRT_COMPOSITOR_LOG=debug XRT_COMPOSITOR_SCALE_PERCENTAGE=${SCALE:-100} PIMAX_LOG=${PIMAX_LOG:-info}
export XR_RUNTIME_JSON=$M/openxr_monado-dev.json
APP=${XR_APP:-hello_xr}
APP_ARGS=${XR_APP_ARGS:--g Vulkan2 -s Local}

echo "*** Keep the headset still for 3 seconds (gyro auto-calibration) ***"
(sleep $((SECS+15)) | $M/src/xrt/targets/service/monado-service >"$OUT/monado.log" 2>&1) &
SVC=$!
sleep 3
python3 ${PIMAX_REPO:-$HOME/omarchyvr}/linux/pimax_logtail.py $((SECS+5)) >"$OUT/fwlog.txt" 2>&1 &
TAIL=$!
sleep 1
echo "*** Starting scene ($APP $APP_ARGS) for ${SECS}s ***"
timeout -s INT "$SECS" $APP $APP_ARGS >"$OUT/client.log" 2>&1
echo "scene exited rc=$?"
pkill -x -INT monado-service; sleep 2
kill $TAIL 2>/dev/null; wait 2>/dev/null
grep -m3 -i 'Panel ON' "$OUT/fwlog.txt" || echo "(no 'Panel ON' in firmware log)"
echo "logs in $OUT"
