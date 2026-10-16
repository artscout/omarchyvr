#!/bin/bash
# Lens tuning session in DIRECT mode on the headset: Monado + xr_grid. Edit ~/.config/pimax/lens.conf (or set
# PIMAX_LENS_CONF), run again, look. See analysis/lens-distortion.md.
# Usage: SECS=60 [FPS=72] [PIMAX_LENS_CONF=file] run_lens_tune.sh [outdir]
OUT=${1:-/tmp/pimax-lens-tune}; mkdir -p "$OUT"
SECS=${SECS:-60}
M=$HOME/src/monado/build
export XRT_COMPOSITOR_FORCE_WAYLAND_DIRECT=1 XRT_COMPOSITOR_WAYLAND_CONNECTOR=DP-1 XRT_COMPOSITOR_PRINT_MODES=1
export XRT_COMPOSITOR_FORCE_NVIDIA=0 XRT_COMPOSITOR_DESIRED_MODE=2
export XRT_COMPOSITOR_LOG=debug XRT_COMPOSITOR_SCALE_PERCENTAGE=${SCALE:-100} PIMAX_LOG=info XRT_LOG=info
export XR_RUNTIME_JSON=$M/openxr_monado-dev.json
[ -n "$PIMAX_LENS_CONF" ] && export PIMAX_LENS_CONF
(sleep $((SECS + 20)) | $M/src/xrt/targets/service/monado-service >"$OUT/monado.log" 2>&1) &
sleep 3
XR_GRID_SECONDS=$SECS XR_GRID_DIST=${DIST:-2.0} timeout $((SECS + 15)) $HOME/pimax-re/analysis/tools/xr_grid >"$OUT/client.log" 2>&1
pkill -x -INT monado-service; sleep 2
grep "pimax lens" "$OUT/monado.log" | head -8
echo logs in $OUT
