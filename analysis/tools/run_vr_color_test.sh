#!/bin/bash
# Colour pipeline check (opens the headset and the display!): one monado-service session, two clients with the same
# test pattern, one with an sRGB swapchain (xr_eyetest_srgb) and one with a UNORM swapchain (xr_eyetest); the final
# compositor image (what goes to the display) is dumped for one frame of each and summarised with `pimax vr dump-stats`.
# Left eye background of the pattern is (0,0x20,0x80) = 0,32,128, right eye (0x80,0x20,0x20) = 128,32,32.
#
# Usage:  run_vr_color_test.sh OUTDIR [MONADO_BUILD]
#   MONADO_BUILD  default ~/src/monado/build; use ~/src/monado-polish/build for the patched compositor
# Env:    PIMAX_BIN (default pimax), SECS (8 per client), SCALE (50: smaller client swapchains, xr_eyetest copies them
#         from the CPU), anything else is passed to monado-service, e.g. XRT_COMPOSITOR_OUTPUT_GAMMA=1.2,
#         XRT_COMPOSITOR_TARGET_FORMAT=B8G8R8A8_UNORM, XRT_COMPOSITOR_COMPUTE=0
# Overnight rule: hold the headset lock:  flock -w 3600 /tmp/pimax-headset.lock run_vr_color_test.sh /tmp/out
set -u
OUT=${1:?usage: run_vr_color_test.sh OUTDIR [MONADO_BUILD]}
BUILD=${2:-$HOME/src/monado/build}
PIMAX=${PIMAX_BIN:-pimax}
SECS=${SECS:-8}
T=$HOME/pimax-re/analysis/tools
mkdir -p "$OUT/dump"
export SCALE=${SCALE:-50}

say() { echo "[$(date +%T)] $*" | tee -a "$OUT/test.log"; }
trap '"$PIMAX" vr stop --no-notify >>"$OUT/stop-trap.txt" 2>&1' EXIT
trap 'say interrupted; exit 130' INT TERM

say "=== colour test, build $BUILD, gamma=${XRT_COMPOSITOR_OUTPUT_GAMMA:-unset} format=${XRT_COMPOSITOR_TARGET_FORMAT:-unset} compute=${XRT_COMPOSITOR_COMPUTE:-unset} ==="
say "usb: $(lsusb | grep 34a4:0044 || echo MISSING)"

"$PIMAX" vr start --fps 72 --monado-build "$BUILD" --dump "$OUT/dump" --dump-frames 100,800 --no-notify >"$OUT/start.txt" 2>&1
RC=$?
cat "$OUT/start.txt"
[ $RC -eq 0 ] || { say "START FAILED rc=$RC"; exit 1; }
sleep 3

export XR_RUNTIME_JSON="$BUILD/openxr_monado-dev.json" IPC_IGNORE_VERSION=1 XR_EYETEST_SECONDS=$SECS
for c in xr_eyetest_srgb xr_eyetest; do
	say "client $c"
	timeout -s INT $((SECS + 25)) "$T/$c" >"$OUT/$c.log" 2>&1
	say "$c rc=$? ($(tail -1 "$OUT/$c.log"))"
	sleep 1
done

"$PIMAX" vr stop --no-notify >"$OUT/stop.txt" 2>&1
STOP_RC=$?
cat "$OUT/stop.txt"
say "stop rc=$STOP_RC; usb: $(lsusb | grep 34a4:0044 || echo MISSING)"
cp /tmp/pimax-vr-session/monado.log "$OUT/monado.log" 2>/dev/null
grep -E "dumped|COMPUTE|target format|imageFormat" "$OUT/monado.log" | head -8 | tee -a "$OUT/test.log"

for f in "$OUT"/dump/frame*.raw; do
	[ -e "$f" ] || { say "NO DUMP FILES"; break; }
	"$PIMAX" vr dump-stats "$f" --step 8 --probe 1000,1900 --probe 6000,1900 | tee -a "$OUT/test.log"
done
say "=== done ==="
