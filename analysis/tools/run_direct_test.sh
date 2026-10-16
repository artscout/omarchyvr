#!/bin/bash
# Direct-mode smoke test (max ~40 s). Usage: run_direct_test.sh [outdir]
OUT=${1:-/tmp/omarchyvr-direct-test}; mkdir -p "$OUT"
M=$HOME/src/monado/build
export XRT_COMPOSITOR_FORCE_WAYLAND_DIRECT=1 XRT_COMPOSITOR_WAYLAND_CONNECTOR=DP-1 XRT_COMPOSITOR_PRINT_MODES=1
export XRT_COMPOSITOR_FORCE_NVIDIA=0 XRT_COMPOSITOR_DESIRED_MODE=${XRT_COMPOSITOR_DESIRED_MODE:--1}
export XRT_COMPOSITOR_LOG=debug XRT_COMPOSITOR_SCALE_PERCENTAGE=${SCALE:-100} PIMAX_LOG=info
export XR_RUNTIME_JSON=$M/openxr_monado-dev.json
(sleep 40 | $M/src/xrt/targets/service/monado-service >"$OUT/monado.log" 2>&1) &
sleep 3
python3 $HOME/pimax-re/linux/pimax_logtail.py 30 >"$OUT/fwlog.txt" 2>&1 &
XR_EYETEST_SECONDS=${SECS:-25} timeout 35 ${XR_CLIENT:-$HOME/pimax-re/analysis/tools/xr_eyetest} >"$OUT/client.log" 2>&1
wait %2 2>/dev/null
pkill -x -INT monado-service; sleep 2
echo logs in $OUT
