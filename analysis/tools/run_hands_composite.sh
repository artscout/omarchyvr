#!/bin/bash
# Offline desk-mode composite: what the compositor puts on the panel with the hands, with NO headset, NO window and NO
# display. Glue only (the pieces are Monado, rust/omarchyvr-passthrough, WayVR and rust/pimax-lenstest dump2png).
#
#   monado-service   debug-image target (WAYLAND_DISPLAY points nowhere, DISPLAY unset, deferred targets off), so it can
#                    neither lease DP-1 nor open a window; PIMAX_NO_DEVICE=1 (no USB) + PIMAX_CTRL_HANDS_DEMO=1 (scripted
#                    hands, head pose identity = looking straight ahead); XRT_COMPOSITOR_DUMP_* writes panel frames
#   omarchyvr-passthrough  main OpenXR app on a .pxcam replay (the room view, as in `omarchyvr desk start`)
#   wayvr --headless   XR_EXTX_overlay on top (no desktop access, so nothing on your desktop moves); config copied
#
#   run_hands_composite.sh                 3 dumps -> $OUT/png/*.png (both eyes side by side, upright)
#   NO_PASSTHROUGH=1 run_hands_composite.sh   WayVR alone (its skybox behind the hands)
# Environment: MONADO_BUILD (default ~/src/monado-eye/build), WAYVR, PASSTHROUGH, LENSTEST, PXCAM, OUT, SECS (warm-up, 8),
#   plus anything Monado/pimax reads (PIMAX_CTRL_*, PIMAX_LENS_CONF ...), passed through.
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
M=${MONADO_BUILD:-$HOME/src/monado-eye/build}
SVC=$M/src/xrt/targets/service/monado-service
RT=$M/openxr_monado-dev.json
WAYVR=${WAYVR:-$HOME/src/target-desk/release/wayvr}
PT=${PASSTHROUGH:-$HERE/../../rust/target/release/omarchyvr-passthrough}
LENSTEST=${LENSTEST:-$HERE/../../rust/target/release/pimax-lenstest}
PXCAM=${PXCAM:-$HERE/../../rust/pimax-cam/tests/fixtures/evening2.pxcam}
WAYVR_HOME_SRC=${WAYVR_HOME_SRC:-$HOME/src/desk-out/wayvr-home-am}
OUT=${OUT:-/tmp/pimax-hands-composite}
SECS=${SECS:-8}
RUNDIR=${XDG_RUNTIME_DIR:-/run/user/$UID}

die() { echo "run_hands_composite: $*" >&2; exit 1; }
for f in "$SVC" "$RT" "$WAYVR" "$LENSTEST"; do [ -e "$f" ] || die "missing $f"; done
[ "${NO_PASSTHROUGH:-0}" = 1 ] || { [ -x "$PT" ] && [ -r "$PXCAM" ]; } || die "missing $PT or $PXCAM (or NO_PASSTHROUGH=1)"
pgrep -x monado-service >/dev/null && die "a monado-service is running (a real session?): not touching it"
pgrep -x wayvr >/dev/null && die "a wayvr is running: not touching it"

rm -rf "$OUT"; mkdir -p "$OUT/raw" "$OUT/png" "$OUT/wayvr-home"
cp -r "$WAYVR_HOME_SRC/." "$OUT/wayvr-home/"   # WayVR saves its state at exit: never into the real desk config

PIDS=()
cleanup() {
	# SIGINT in reverse start order, wait; Monado last (it has no display here, but keep the clean-stop habit)
	for ((i = ${#PIDS[@]} - 1; i >= 0; i--)); do
		p=${PIDS[$i]}
		kill -INT "$p" 2>/dev/null || continue
		for _ in $(seq 100); do kill -0 "$p" 2>/dev/null || break; sleep 0.1; done
		kill -0 "$p" 2>/dev/null && { echo "pid $p did not stop in 10 s, SIGTERM"; kill -TERM "$p" 2>/dev/null; }
	done
}
trap cleanup EXIT
trap 'exit 130' INT TERM

rm -f "$RUNDIR/monado_comp_ipc"
env -u DISPLAY WAYLAND_DISPLAY=pimax-hands-composite-none XRT_COMPOSITOR_DISABLE_DEFERRED=1 \
	PIMAX_NO_DEVICE=1 PIMAX_CTRL_HANDS=1 PIMAX_CTRL_HANDS_DEMO=1 PIMAX_EYE=0 XRT_NO_STDIN=1 \
	XRT_COMPOSITOR_DUMP_DIR="$OUT/raw" XRT_COMPOSITOR_DUMP_TRIGGER="$OUT/raw/trigger" \
	XRT_LOG=${XRT_LOG:-info} PIMAX_LOG=${PIMAX_LOG:-info} \
	"$SVC" </dev/null >"$OUT/monado.log" 2>&1 &
PIDS+=($!)
for _ in $(seq 80); do
	[ -S "$RUNDIR/monado_comp_ipc" ] && break
	kill -0 "${PIDS[0]}" 2>/dev/null || { tail -20 "$OUT/monado.log"; die "monado-service exited at start-up"; }
	sleep 0.25
done
for _ in $(seq 60); do grep -q "Debug image target used" "$OUT/monado.log" && break; sleep 0.25; done
grep -q "Debug image target used" "$OUT/monado.log" || { grep -iE "target|backend|lease" "$OUT/monado.log" | head; die "not on the debug-image target: stopping"; }

export XR_RUNTIME_JSON=$RT IPC_IGNORE_VERSION=1
if [ "${NO_PASSTHROUGH:-0}" != 1 ]; then
	"$PT" run --source "$PXCAM" >"$OUT/passthrough.log" 2>&1 </dev/null &
	PIDS+=($!)
	sleep 2
fi
XDG_CONFIG_HOME=$OUT/wayvr-home/config XDG_DATA_HOME=$OUT/wayvr-home/data XDG_STATE_HOME=$OUT/wayvr-home/state \
	RUST_LOG=info "$WAYVR" --openxr --headless --show --no-autostart --log-to "$OUT/wayvr.log" \
	>"$OUT/wayvr.stdout" 2>&1 </dev/null &
PIDS+=($!)

sleep "$SECS"
for n in 1 2 3; do
	touch "$OUT/raw/trigger"
	for _ in $(seq 50); do [ -e "$OUT/raw/trigger" ] || break; sleep 0.1; done
	sleep 1.3   # the demo pattern moves the fingers: three different poses
done
cleanup; PIDS=(); trap - EXIT

for r in "$OUT"/raw/frame*.raw; do
	[ -e "$r" ] || die "no dumps written (see $OUT/monado.log)"
	"$LENSTEST" dump2png "$r" "$OUT/png/$(basename "${r%.raw}").png" --down 4 || die "dump2png failed on $r"
done
grep -hE "hand (left|right):|Hand tracking" "$OUT/wayvr.stdout" | sed 's/\x1b\[[0-9;]*m//g' | tail -4
echo "PNGs (left eye | right eye, upright): $OUT/png"
