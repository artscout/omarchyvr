#!/bin/bash
# Launcher + refresh-rate test (opens the headset and the display!).
# `pimax vr start --fps N --debug --dump` -> OpenXR client for SECS seconds while the firmware log is tailed
# -> `pimax vr status` -> `pimax vr stop` (SIGINT + kernel log check) -> numbers about the dumped frames.
#
# Usage:  run_vr_fps_test.sh FPS OUTDIR [CLIENT]
#   FPS     72 or 90
#   OUTDIR  logs and dumps go here (created)
#   CLIENT  OpenXR test client, default analysis/tools/xr_room (GPU drawn; xr_eyetest copies 214 MB per frame)
# Env:    PIMAX_BIN (default: pimax), PIMAX_MONADO_BUILD (default ~/src/monado/build), SECS (20),
#         DUMP_FRAMES (200,600), SCALE (XRT_COMPOSITOR_SCALE_PERCENTAGE, passed on)
# Overnight rule: hold the headset lock while this runs:
#   flock -w 3600 /tmp/pimax-headset.lock analysis/tools/run_vr_fps_test.sh 90 /tmp/pimax-fps-90
set -u
FPS=${1:?usage: run_vr_fps_test.sh FPS OUTDIR [CLIENT]}
OUT=${2:?usage: run_vr_fps_test.sh FPS OUTDIR [CLIENT]}
CLIENT=${3:-$HOME/pimax-re/analysis/tools/xr_room}
PIMAX=${PIMAX_BIN:-pimax}
SECS=${SECS:-20}
FRAMES=${DUMP_FRAMES:-200,600}
BUILD=${PIMAX_MONADO_BUILD:-$HOME/src/monado/build}
mkdir -p "$OUT/dump"

say() { echo "[$(date +%T)] $*" | tee -a "$OUT/test.log"; }
TAIL_PID=""
cleanup() {
	# no service may outlive this script; `vr stop` is a no-op when it was stopped already
	"$PIMAX" vr stop --no-notify >>"$OUT/stop-trap.txt" 2>&1
	[ -n "$TAIL_PID" ] && kill "$TAIL_PID" 2>/dev/null
}
trap cleanup EXIT
trap 'say "interrupted"; exit 130' INT TERM

say "=== $FPS Hz test, client $(basename "$CLIENT"), ${SECS}s, pimax=$PIMAX ==="
say "usb: $(lsusb | grep 34a4:0044 || echo MISSING)"
KERN0=$(journalctl -k -b --no-pager | grep -cE "Xid|Idling")
say "kernel Xid/Idling lines before: $KERN0"

# firmware + display-bridge log, passive (sends nothing)
timeout $((SECS + 45)) "$PIMAX" logtail $((SECS + 35)) >"$OUT/fwlog.txt" 2>&1 &
TAIL_PID=$!

say "pimax vr start --fps $FPS"
"$PIMAX" vr start --fps "$FPS" --debug --dump "$OUT/dump" --dump-frames "$FRAMES" >"$OUT/start.txt" 2>&1
RC=$?
cat "$OUT/start.txt"
say "start rc=$RC"
if [ $RC -ne 0 ]; then
	say "START FAILED"
	exit 1
fi
sleep 8 # the panels light about 6 s after the video stream starts

export XR_RUNTIME_JSON="$BUILD/openxr_monado-dev.json" IPC_IGNORE_VERSION=1
export XR_ROOM_SECONDS=$SECS XR_EYETEST_SECONDS=$SECS
say "client: $CLIENT"
timeout -s INT $((SECS + 25)) stdbuf -oL "$CLIENT" 2>&1 |
	while IFS= read -r l; do echo "$(date +%T.%N | cut -c1-12) $l"; done >"$OUT/client.log"
say "client rc=${PIPESTATUS[0]}"
tail -4 "$OUT/client.log"

"$PIMAX" vr status --no-notify >"$OUT/status-after-client.txt" 2>&1
say "status rc=$?"
sleep 2
"$PIMAX" vr stop --no-notify >"$OUT/stop.txt" 2>&1
STOP_RC=$?
cat "$OUT/stop.txt"
say "stop rc=$STOP_RC"

say "usb after: $(lsusb | grep 34a4:0044 || echo MISSING)"
KERN1=$(journalctl -k -b --no-pager | grep -cE "Xid|Idling")
say "kernel Xid/Idling lines after: $KERN1 (before: $KERN0)"
journalctl -k -b --no-pager | grep -E "Xid|Idling|nvidia-modeset" | tail -5 >"$OUT/kernel-tail.txt"

"$PIMAX" vr status --no-notify --log /tmp/pimax-vr-session/monado.log >"$OUT/summary.txt" 2>&1
cp /tmp/pimax-vr-session/monado.log "$OUT/monado.log" 2>/dev/null
say "--- monado log summary ---"
tee -a "$OUT/test.log" <"$OUT/summary.txt"

say "--- firmware log: refresh / panel lines ---"
grep -E "fps calc|required fps|requiredFps|Panel ON|panel is off|WORK_ON|WAIT_VIDEO_STABLE|No video input|power off|fps_stable" "$OUT/fwlog.txt" | head -40 | tee -a "$OUT/test.log"

say "--- dumps ---"
ls -la "$OUT/dump" | tee -a "$OUT/test.log"
for f in "$OUT"/dump/frame*.raw; do
	[ -e "$f" ] || { say "NO DUMP FILES"; break; }
	"$PIMAX" vr dump-stats "$f" --step 8 --probe 1000,1900 --probe 6000,1900 --probe 1776,1920 --ppm "${f%.raw}.ppm" | tee -a "$OUT/test.log"
done
say "=== done ==="
exit 0
