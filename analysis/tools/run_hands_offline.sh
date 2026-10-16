#!/bin/bash
# Offline camera-hands run: monado-service (Mercury build) on a .pxcam replay with NO headset access, NO display and
# its OWN runtime/config dirs, so a live desk session on this machine is not touched (different IPC socket, the
# pimax USB driver disabled, PIMAX_NO_DEVICE=1, null compositor). Optionally an OpenXR client checks the hands.
#
#   run_hands_offline.sh                      30 s, PIMAX_HANDS=mercury, xr-handtest (any source) against it
#   CLIENT=pinch PIMAX_HT_ROLES=hands run_hands_offline.sh   xr-pinchtest: hand_interaction_ext bound to the hands
#   NO_MODELS=1 run_hands_offline.sh          models missing: hands must be off, the service must keep running
#   CLIENT=none SECS=60 run_hands_offline.sh  service only (CPU / frame stats)
#
# Environment: MONADO_BUILD (default ~/.local/lib/omarchyvr/monado), PXCAM (default ~/ctrl-test.pxcam), SECS (30),
#   OUT (default /tmp/pimax-hands-offline), CLIENT (hand|pinch|none), plus PIMAX_HT_* / PIMAX_* passed through.
# Output: $OUT/monado.log, $OUT/client.log, $OUT/cpu.txt (per-thread CPU of the service), $OUT/summary.txt
set -u
M=${MONADO_BUILD:-$HOME/.local/lib/omarchyvr/monado}
SVC=$M/src/xrt/targets/service/monado-service
RT=$M/openxr_monado-dev.json
PXCAM=${PXCAM:-$HOME/ctrl-test.pxcam}
SECS=${SECS:-30}
OUT=${OUT:-/tmp/pimax-hands-offline}
CLIENT=${CLIENT:-hand}
TOOLS=${TOOLS:-${PIMAX_REPO:-$HOME/omarchyvr}/rust/target/release}

die() { echo "run_hands_offline: $*" >&2; exit 1; }
for f in "$SVC" "$RT" "$PXCAM"; do [ -e "$f" ] || die "missing $f"; done

rm -rf "$OUT"
mkdir -p "$OUT/run" "$OUT/config/monado" "$OUT/data"
chmod 700 "$OUT/run"
# Private Monado config: the pimax USB driver is disabled, so the real headset (if plugged in) is never opened; the
# PIMAX_NO_DEVICE auto prober creates the HMD without a device instead.
cat >"$OUT/config/monado/config_v0.json" <<'EOF'
{ "disabled": ["pimax", "psvr2", "hydra", "psmv", "osvr", "rokid", "blubur_s1", "contactglove"] }
EOF
DATA_ENV=()
if [ "${NO_MODELS:-0}" = 1 ]; then
	mkdir -p "$OUT/data/monado/hand-tracking-models" # exists but empty: the driver must refuse cleanly
	DATA_ENV=(XDG_DATA_HOME="$OUT/data")
fi

SVC_PID=
cleanup() {
	if [ -n "$SVC_PID" ] && kill -0 "$SVC_PID" 2>/dev/null; then
		kill -INT "$SVC_PID"
		for _ in $(seq 100); do kill -0 "$SVC_PID" 2>/dev/null || break; sleep 0.1; done
		kill -0 "$SVC_PID" 2>/dev/null && kill -TERM "$SVC_PID"
	fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM

env -u DISPLAY -u WAYLAND_DISPLAY XDG_RUNTIME_DIR="$OUT/run" XDG_CONFIG_HOME="$OUT/config" "${DATA_ENV[@]}" \
	XRT_COMPOSITOR_NULL=1 XRT_NO_STDIN=1 PIMAX_NO_DEVICE=1 PIMAX_EYE=0 \
	PIMAX_HANDS=${PIMAX_HANDS:-mercury} PIMAX_HT_STATS=${PIMAX_HT_STATS:-1} PIMAX_CAM_SOURCE="$PXCAM" \
	XRT_LOG=${XRT_LOG:-info} \
	nice -n "${SVC_NICE:-10}" "$SVC" </dev/null >"$OUT/monado.log" 2>&1 &
SVC_PID=$!
for _ in $(seq 100); do
	[ -S "$OUT/run/monado_comp_ipc" ] && break
	kill -0 "$SVC_PID" 2>/dev/null || { tail -30 "$OUT/monado.log"; die "monado-service exited at start-up"; }
	sleep 0.1
done
[ -S "$OUT/run/monado_comp_ipc" ] || die "no IPC socket after 10 s"

CLIENT_RC=-
case "$CLIENT" in
hand)
	XDG_RUNTIME_DIR="$OUT/run" XR_RUNTIME_JSON="$RT" "$TOOLS/xr-handtest" --source any --no-grip-check \
		--require any --min-valid 0 --seconds $((SECS - 5)) >"$OUT/client.log" 2>&1
	CLIENT_RC=$?
	;;
pinch)
	XDG_RUNTIME_DIR="$OUT/run" XR_RUNTIME_JSON="$RT" "$TOOLS/xr-pinchtest" --seconds $((SECS - 5)) \
		>"$OUT/client.log" 2>&1
	CLIENT_RC=$?
	;;
none) ;;
*) die "CLIENT=$CLIENT (hand, pinch or none)" ;;
esac &
CPID=$!
sleep 3
[ -x "$TOOLS/pimax-cam" ] && "$TOOLS/pimax-cam" cpu "$SVC_PID" --seconds $((SECS - 8)) >"$OUT/cpu.txt" 2>&1
wait $CPID
ALIVE=no
kill -0 "$SVC_PID" 2>/dev/null && ALIVE=yes
cleanup
wait "$SVC_PID" 2>/dev/null
SVC_RC=$?
{
	echo "service alive at the end: $ALIVE, exit $SVC_RC"
	grep -E "hands|Mercury|camera: " "$OUT/monado.log" | tail -12
	[ -f "$OUT/client.log" ] && tail -6 "$OUT/client.log"
} | tee "$OUT/summary.txt"
