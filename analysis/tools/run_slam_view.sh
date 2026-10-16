#!/bin/bash
# Live 6DoF walk with the camera view in the headset, in one command:
#   camera broker (one camera stream shared) -> Monado SLAM (run_slam.sh live, reads the broker)
#   -> omarchyvr-passthrough (reads the broker) so you see where you walk.
#
#   run_slam_view.sh [SECS]     default 90 s. Env: PIMAX_CAM_AE (default 0), PXCAM (raw .pxcam of all frames), M (Monado build, default ~/.local/lib/omarchyvr/monado)
# Stops everything it started (by PID) at the end and prints a short check of the run.
set -u
SECS=${1:-90}
export PIMAX_CAM_AE=${PIMAX_CAM_AE:-0}
M=${M:-$HOME/.local/lib/omarchyvr/monado}
T=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
BIN=$HOME/.local/bin
RECROOT=$HOME/.local/share/omarchyvr/slam-recordings
SOCK=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/omarchyvr-cam-broker.sock
LOGD=$(mktemp -d /tmp/pimax-slamview.XXXX)
BROKER=; PT=; SLAM=

die() { echo "!!! $*"; cleanup; exit 1; }
cleanup() {
	[ -n "$PT" ] && kill -INT "$PT" 2>/dev/null
	[ -n "$SLAM" ] && kill -INT "$SLAM" 2>/dev/null && wait "$SLAM" 2>/dev/null
	sleep 1
	[ -n "$PT" ] && kill "$PT" 2>/dev/null
	[ -n "${REC:-}" ] && kill -INT "$REC" 2>/dev/null && wait "$REC" 2>/dev/null
	[ -n "$BROKER" ] && kill -INT "$BROKER" 2>/dev/null
	sleep 1
	[ -n "$BROKER" ] && kill "$BROKER" 2>/dev/null
	PT=; SLAM=; BROKER=
}
trap 'echo; echo "interrupted"; cleanup; exit 130' INT TERM

# 1. Preflight: nothing may hold the camera or run Monado already.
pgrep -x monado-service >/dev/null && die "monado-service is already running (pgrep -a monado); stop it first (omarchyvr desk stop / omarchyvr vr stop --grace 30)"
for v in /dev/video*; do fuser "$v" >/dev/null 2>&1 && die "$v is busy: $(fuser -v "$v" 2>&1 | tail -n +2 | tr -s ' ')"; done
for b in pimax-cam omarchyvr-passthrough; do [ -x "$BIN/$b" ] || die "missing $BIN/$b (run ./install.sh again in your omarchyvr clone, or: cd ~/omarchyvr/rust && cargo build --release -p $b)"; done
rm -f "$SOCK"

# 2. Camera broker.
"$BIN/pimax-cam" broker >"$LOGD/broker.log" 2>&1 &
BROKER=$!
for _ in $(seq 50); do [ -S "$SOCK" ] && break; kill -0 "$BROKER" 2>/dev/null || break; sleep 0.2; done
[ -S "$SOCK" ] || die "broker did not come up: $(tail -3 "$LOGD/broker.log")"
echo "[1/3] camera broker up (pid $BROKER)"
# Optional raw recording of every camera frame (all 4 cameras, tracking + LED frames): PXCAM=/path/out.pxcam
REC=
if [ -n "${PXCAM:-}" ]; then
	"$BIN/pimax-cam" record "$PXCAM" --seconds "$((SECS + 15))" --zlib 1 --source broker >"$LOGD/record.log" 2>&1 &
	REC=$!
	echo "      raw recording to $PXCAM (pid $REC)"
fi

# 3. Monado SLAM + pose printer, reading the broker.
BEFORE=$(ls -d "$RECROOT"/*-live 2>/dev/null | tail -n 1)
POSECLI_OVERLAY=1 POSE_DELAY=12 PIMAX_CAM_SOURCE=broker M=$M "$T/run_slam.sh" live "$SECS" 2>&1 | tee "$LOGD/slam.log" &
SLAM=$!
OUT=
for _ in $(seq 100); do
	OUT=$(ls -d "$RECROOT"/*-live 2>/dev/null | tail -n 1); [ "$OUT" != "$BEFORE" ] && [ -f "$OUT/monado.log" ] && break
	OUT=; sleep 0.2
done
[ -n "$OUT" ] || die "run_slam.sh did not start"
for _ in $(seq 75); do
	grep -q "camera pipeline up" "$OUT/monado.log" 2>/dev/null && break
	grep -qE "camera: open|already running|init_all failed" "$OUT/monado.log" 2>/dev/null && die "Monado failed: $(grep -E 'camera: open|already running|init_all failed' "$OUT/monado.log" | head -2)"
	sleep 0.2
done
grep -q "setup_slam" "$OUT/monado.log" || die "SLAM did not start (see $OUT/monado.log)"
echo "[2/3] Monado SLAM up, recording to $OUT"

# 4. Passthrough view (stdin kept open so it does not see EOF). It must connect before the pose printer
#    (POSE_DELAY=12 above): Monado shows only the first app that becomes active.
XR_RUNTIME_JSON=$M/openxr_monado-dev.json "$BIN/omarchyvr-passthrough" run --source broker \
	< <(tail -f /dev/null) >"$LOGD/passthrough.log" 2>&1 &
PT=$!
sleep 3
if [ -n "$PT" ] && kill -0 "$PT" 2>/dev/null; then
	echo "[3/3] camera view up (pid $PT). PUT THE HEADSET ON: stand still 5 s, walk slowly, end where you started."
else
	echo "[3/3] camera view FAILED (walk still records): $(tail -3 "$LOGD/passthrough.log")"; PT=
fi

# 5. Wait for the walk to end, then stop everything.
wait "$SLAM"; SLAM=
cleanup

# 6. Short check.
echo
echo "=== result ($OUT) ==="
P=$OUT/poses.txt
echo "pose lines: $(wc -l <"$P" 2>/dev/null), with 6dof: $(grep -c 6dof "$P" 2>/dev/null), last: $(tail -n 1 "$P" 2>/dev/null)"
E=$(ls -d "$OUT"/euroc_* 2>/dev/null | tail -n 1)
[ -n "$E" ] && echo "recorded: imu $(($(wc -l <"$E/mav0/imu0/data.csv") - 1)) rows, cam0 $(($(wc -l <"$E/mav0/cam0/data.csv") - 1)) frames"
echo "errors: $(grep -cE 'ERROR' "$OUT/monado.log" 2>/dev/null) in monado.log (excluding allowlist: $(grep 'ERROR' "$OUT/monado.log" | grep -vc allowlisted))"
echo "new kernel Xid: $(journalctl -k -b --since "-$((SECS + 60)) s" 2>/dev/null | grep -ci xid)"
echo "logs: $LOGD (broker, slam, passthrough); replay: $T/run_slam.sh replay $OUT"
