#!/bin/bash
# Controller-driven hand tracking test: Monado service (null compositor, no display) + xr-handtest (Rust, headless OpenXR).
# Everything runs under the shared headset lock (max ~15 min). Usage:
#   run_hand_test.sh [phase ...]        phases: nodevice demo real   (default: nodevice real)
#     nodevice  PIMAX_NO_DEVICE=1 + scripted controller pattern: the headset is NOT opened; checks the whole
#               OpenXR -> Monado -> Rust path (finger order, left/right, pose composition)
#     demo      real headset open, scripted pattern instead of controller reports
#     real      real headset and the real controllers (awake, lying still): valid joints at rest, hands follow the grips
#   env: MONADO_BUILD (default ~/src/monado-hands/build-hands, else ~/src/monado/build)
#        CARGO_TARGET_DIR (default ~/src/target-hands, else ~/pimax-re/rust/target)
#        XR_HANDTEST (prebuilt client)  NO_BUILD=1  SECS (client seconds per phase, default 10; demo phases 24)
#        POSE=imu (PIMAX_CTRL_POSE for the service)  OUT (log dir)  DRYRUN=1 (take the lock, print what would run)
#        PIMAX_HEADSET_LOCK (lock file, default /tmp/pimax-headset.lock)
# Exit status: 0 all phases passed, 1 a phase failed, 3 kernel Xid / "Idling display engine" seen (stop all headset work).
set -u
LOCK=${PIMAX_HEADSET_LOCK:-/tmp/pimax-headset.lock}

# Defaults: the overnight worktree build if it exists, else the user's build; same idea for the cargo target dir.
if [ -z "${MONADO_BUILD:-}" ]; then
	MONADO_BUILD=$HOME/src/monado-hands/build-hands
	[ -x "$MONADO_BUILD/src/xrt/targets/service/monado-service" ] || MONADO_BUILD=$HOME/src/monado/build
fi
if [ -z "${CARGO_TARGET_DIR:-}" ]; then
	CARGO_TARGET_DIR=$HOME/src/target-hands
	[ -d "$CARGO_TARGET_DIR/release" ] || CARGO_TARGET_DIR=$HOME/pimax-re/rust/target
fi
export CARGO_TARGET_DIR
OUT=${OUT:-/tmp/pimax-hand-test.$$}
SECS=${SECS:-10}
PHASES=("$@")
[ ${#PHASES[@]} -gt 0 ] || PHASES=(nodevice real)
SERVICE=$MONADO_BUILD/src/xrt/targets/service/monado-service
export XR_RUNTIME_JSON=$MONADO_BUILD/openxr_monado-dev.json
mkdir -p "$OUT"

if [ -z "${XR_HANDTEST:-}" ]; then
	XR_HANDTEST=$CARGO_TARGET_DIR/release/xr-handtest
	if [ -z "${NO_BUILD:-}" ]; then
		# other agents add crates to the workspace all the time; a half-written one can break the build for a while
		for try in 1 2 3 4 5 6; do
			(cd "$HOME/pimax-re/rust" && cargo build --release -p pimax-xrtest) >"$OUT/cargo-build.log" 2>&1 && break
			echo "cargo build failed (try $try), retrying"; sleep 10
		done
	fi
fi
for f in "$SERVICE" "$XR_RUNTIME_JSON" "$XR_HANDTEST"; do
	[ -e "$f" ] || { echo "missing $f"; exit 2; }
done

# Build done: now hold the headset lock for the whole run (wait up to an hour for the others). It lives on fd 9; every
# child that may outlive this script (the sleep that keeps the service's stdin open, the service) is started with 9>&-
# so it cannot leak the lock.
exec 9>"$LOCK"
flock -w 3600 9 || { echo "could not get $LOCK within an hour"; exit 3; }
if pgrep -x monado-service >/dev/null; then
	echo "a monado-service is already running (not started by this script) - not touching it"; exit 2
fi

SVC_PID=""; SLEEP_PID=""
stop_service() {
	[ -n "$SVC_PID" ] || return 0
	kill -INT "$SVC_PID" 2>/dev/null
	for _ in $(seq 50); do kill -0 "$SVC_PID" 2>/dev/null || break; sleep 0.1; done
	kill -0 "$SVC_PID" 2>/dev/null && { echo "service did not stop in 5 s, SIGKILL"; kill -KILL "$SVC_PID" 2>/dev/null; }
	wait "$SVC_PID" 2>/dev/null
	[ -n "$SLEEP_PID" ] && kill "$SLEEP_PID" 2>/dev/null
	SVC_PID=""; SLEEP_PID=""
}
trap stop_service EXIT

# 0 ok, 3 kernel complaint
kernel_check() {
	if journalctl -k -b 2>/dev/null | grep -E "Xid|Idling" >"$OUT/kernel-hits.txt"; then
		echo "KERNEL: Xid / Idling display engine seen after phase $1 - STOP all headset work:"; cat "$OUT/kernel-hits.txt"
		{ echo; echo "## $(date '+%F %T') run_hand_test.sh (phase $1)"; cat "$OUT/kernel-hits.txt"; } >>"$HOME/pimax-re/analysis/overnight-incidents.md"
		return 3
	fi
	return 0
}

run_phase() {
	local phase=$1 secs=$SECS flags=() senv=(XRT_COMPOSITOR_NULL=1 PIMAX_LOG=info)
	case $phase in
		nodevice) senv+=(PIMAX_NO_DEVICE=1 PIMAX_CTRL_HANDS_DEMO=1); flags+=(--demo --require both); secs=24 ;;
		demo)     senv+=(PIMAX_CTRL_HANDS_DEMO=1); flags+=(--demo --require both); secs=24 ;;
		real)     flags+=(--require any --require-grip) ;;
		*) echo "unknown phase $phase"; return 2 ;;
	esac
	[ -n "${POSE:-}" ] && senv+=(PIMAX_CTRL_POSE="$POSE")
	echo "=== phase $phase: ${senv[*]} ==="
	local sock=$XDG_RUNTIME_DIR/monado_comp_ipc
	rm -f "$sock"
	if [ -n "${DRYRUN:-}" ]; then echo "(dry run) would start $SERVICE and $XR_HANDTEST --seconds $secs ${flags[*]}"; return 0; fi
	exec 3< <(exec sleep 1500 9>&-)   # keeps the service's stdin open
	SLEEP_PID=$!
	env "${senv[@]}" "$SERVICE" <&3 9>&- >"$OUT/service-$phase.log" 2>&1 &
	SVC_PID=$!
	exec 3<&-
	for _ in $(seq 100); do [ -S "$sock" ] && break; kill -0 "$SVC_PID" 2>/dev/null || break; sleep 0.2; done
	if ! kill -0 "$SVC_PID" 2>/dev/null || ! [ -S "$sock" ]; then
		echo "service did not come up:"; tail -20 "$OUT/service-$phase.log"; stop_service; return 1
	fi
	[ "$phase" = nodevice ] || sleep 4   # controller attach burst + first reports
	"$XR_HANDTEST" --seconds "$secs" "${flags[@]}" 9>&- 2>&1 | tee "$OUT/client-$phase.log"
	local rc=${PIPESTATUS[0]}
	stop_service
	grep -E "Pimax controllers|hand tracking|Created hand tracker|input\[|first report|attach burst|ERROR|WARN" "$OUT/service-$phase.log" | head -20
	kernel_check "$phase" || return 3
	return "$rc"
}

overall=0
for p in "${PHASES[@]}"; do
	run_phase "$p"; rc=$?
	echo "--- phase $p: exit $rc"
	[ "$rc" -eq 3 ] && { overall=3; break; }
	[ "$rc" -ne 0 ] && overall=1
done
echo "logs in $OUT"
exit $overall
