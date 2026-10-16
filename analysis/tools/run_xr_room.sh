#!/bin/bash
# World-locked sky-sphere room in the headset via Monado direct mode. Usage: [SECS=60] run_xr_room.sh [outdir]
OUT=${1:-/tmp/pimax-xr-room}; mkdir -p "$OUT"
SECS=${SECS:-60}
T=$HOME/pimax-re/analysis/tools
M=$HOME/src/monado/build
[ "$T/xr_room" -nt "$T/xr_room.c" ] || gcc -O2 -Wno-misleading-indentation -o "$T/xr_room" "$T/xr_room.c" -lopenxr_loader -lvulkan -lm -ldl || exit 1
export XRT_COMPOSITOR_FORCE_WAYLAND_DIRECT=1 XRT_COMPOSITOR_WAYLAND_CONNECTOR=DP-1 XRT_COMPOSITOR_PRINT_MODES=1
export XRT_COMPOSITOR_FORCE_NVIDIA=0 XRT_COMPOSITOR_DESIRED_MODE=${XRT_COMPOSITOR_DESIRED_MODE:-2}
export XRT_COMPOSITOR_LOG=debug XRT_COMPOSITOR_SCALE_PERCENTAGE=${SCALE:-100} PIMAX_LOG=${PIMAX_LOG:-info}
export XR_RUNTIME_JSON=$M/openxr_monado-dev.json

echo "*** Keep the headset still for 3 seconds (gyro auto-calibration) ***"
(sleep $((SECS+15)) | $M/src/xrt/targets/service/monado-service >"$OUT/monado.log" 2>&1) &
sleep 3
python3 $HOME/pimax-re/linux/pimax_logtail.py $((SECS+5)) >"$OUT/fwlog.txt" 2>&1 &
TAIL=$!
sleep 1
echo "*** Starting xr_room for ${SECS}s ***"
XR_ROOM_SECONDS=$SECS timeout -s INT $((SECS+10)) "$T/xr_room" >"$OUT/client.log" 2>&1
echo "xr_room exited rc=$?"
pkill -x -INT monado-service; sleep 2
kill $TAIL 2>/dev/null; wait 2>/dev/null
grep -m3 -i 'Panel ON' "$OUT/fwlog.txt" || echo "(no 'Panel ON' in firmware log)"
echo "logs in $OUT"
