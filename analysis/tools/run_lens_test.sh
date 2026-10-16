#!/bin/bash
# Lens test session: starts monado-service (direct mode, reads the CURRENT ~/.config/pimax/lens.conf) and runs
# pimax-lenstest, the OpenXR test pattern client. Glue only: the logic is in rust/pimax-lenstest.
#
#   run_lens_test.sh                         guided session in the headset (controllers or keyboard in THIS terminal)
#   run_lens_test.sh --auto 8                same, stepping through everything every 8 s
#   run_lens_test.sh --capture DIR           no wearer needed: show every pattern, save what the compositor puts on the
#                                            panel (after the lens model) as PNGs into DIR
#   run_lens_test.sh --lens-conf F ...       use F instead of ~/.config/pimax/lens.conf (exported as PIMAX_LENS_CONF)
#   all other options go to pimax-lenstest (see: pimax-lenstest --help)
#
# Environment:
#   MONADO_BUILD   Monado build dir (default ~/src/monado/build)
#   LENSTEST       the client binary (default rust/target/release/pimax-lenstest, then ~/src/target-lens/release)
#   NO_DEVICE=1    do not open the headset USB (compositor + panel link only; for captures nobody watches)
#   WINDOWED=1     use a window on the desktop instead of the headset (implies NO_DEVICE=1, scaled down)
#   MODE=N         XRT_COMPOSITOR_DESIRED_MODE (default 2 = 7104x3840@72, see direct-mode-test.md)
#   LOGDIR         where monado.log goes (default /tmp/pimax-lens-test)
#   HEADSET_LOCK=1 take /tmp/pimax-headset.lock (flock -o) for the whole run: use it when other agents are about.
#                  (If you wrap this script in flock yourself, use `flock -o`: see the note at the sleep below.)
# Stops monado-service with SIGINT (waits, then SIGKILL) and checks the kernel log for NVIDIA display errors.

set -u
HERE=$(cd "$(dirname "$0")" && pwd)

if [ "${HEADSET_LOCK:-0}" = 1 ] && [ -z "${_LENS_LOCKED:-}" ]; then
	# -o: children (the sleep that feeds monado-service its stdin) must not inherit the lock descriptor, or a
	# leftover sleep would hold the headset lock after this script has ended
	_LENS_LOCKED=1 exec flock -o -w 3600 /tmp/pimax-headset.lock "$0" "$@"
fi

M=${MONADO_BUILD:-$HOME/src/monado/build}
SVC=$M/src/xrt/targets/service/monado-service
RT=$M/openxr_monado-dev.json
APP=${LENSTEST:-}
if [ -z "$APP" ]; then
	for c in "$HERE/../../rust/target/release/pimax-lenstest" "$HOME/src/target-lens/release/pimax-lenstest"; do
		[ -x "$c" ] && { APP=$c; break; }
	done
fi
LOGDIR=${LOGDIR:-/tmp/pimax-lens-test}
mkdir -p "$LOGDIR"

die() { echo "run_lens_test: $*" >&2; exit 1; }
[ -x "$SVC" ] || die "no monado-service at $SVC (set MONADO_BUILD)"
[ -r "$RT" ] || die "no OpenXR runtime json at $RT (set MONADO_BUILD)"
[ -n "$APP" ] && [ -x "$APP" ] || die "pimax-lenstest not built: (cd ~/pimax-re/rust && cargo build --release -p pimax-lenstest), or set LENSTEST"
if pgrep -x monado-service >/dev/null; then die "a monado-service is already running; stop it first (Ctrl-C in its terminal)"; fi

# options of this script; everything else goes to the client
ARGS=()
CAPTURE=""
LENS_CONF=""
while [ $# -gt 0 ]; do
	case "$1" in
		--capture) CAPTURE=$2; ARGS+=("--capture" "$2"); shift 2 ;;
		--lens-conf) LENS_CONF=$2; ARGS+=("--lens-conf" "$2"); shift 2 ;;
		*) ARGS+=("$1"); shift ;;
	esac
done
[ -n "$LENS_CONF" ] && export PIMAX_LENS_CONF=$LENS_CONF

export XR_RUNTIME_JSON=$RT IPC_IGNORE_VERSION=1
export PIMAX_LOG=${PIMAX_LOG:-info} XRT_LOG=${XRT_LOG:-info} XRT_COMPOSITOR_LOG=${XRT_COMPOSITOR_LOG:-info}
export XRT_COMPOSITOR_SCALE_PERCENTAGE=${SCALE:-100}
if [ "${WINDOWED:-0}" = 1 ]; then
	export XRT_COMPOSITOR_FORCE_XCB=1 PIMAX_NO_DEVICE=1 PIMAX_DEBUG_DIV=${PIMAX_DEBUG_DIV:-4}
else
	export XRT_COMPOSITOR_FORCE_WAYLAND_DIRECT=1 XRT_COMPOSITOR_WAYLAND_CONNECTOR=${CONNECTOR:-DP-1}
	export XRT_COMPOSITOR_FORCE_NVIDIA=0 XRT_COMPOSITOR_DESIRED_MODE=${MODE:-2}
	[ "${NO_DEVICE:-0}" = 1 ] && export PIMAX_NO_DEVICE=1
fi
if [ -n "$CAPTURE" ]; then
	mkdir -p "$CAPTURE/raw"
	export XRT_COMPOSITOR_DUMP_DIR=$CAPTURE/raw XRT_COMPOSITOR_DUMP_TRIGGER=$CAPTURE/raw/trigger
fi

cat <<TXT
=== lens test session ===
monado:  $SVC
client:  $APP
lens.conf: ${PIMAX_LENS_CONF:-$HOME/.config/pimax/lens.conf}   (read once when monado-service starts: edit it, then run this again)
log:     $LOGDIR/monado.log   (grep "pimax lens" for what the driver loaded)
TXT
"$APP" config ${LENS_CONF:+--lens-conf "$LENS_CONF"} 2>/dev/null | head -3
if [ -z "$CAPTURE" ]; then
	cat <<TXT

In the headset: A/X next pattern, B/Y previous, stick left/right step (square position, colour channel,
border step), stick up/down square direction, stick click eye (both/left/right), trigger auto on/off,
grip text on/off. Controllers sleep until a button is pressed. Keys in this terminal also work:
n p . , e a h d 1-7 q.  Look for what the text in the headset says; the same text is printed here and
listed by: $APP list
Fill in analysis/lens-test-checklist.md while you look.
TXT
fi

XID0=$(journalctl -k -b 2>/dev/null | grep -cE "Xid|Idling display engine")

# monado-service wants a pollable stdin: feed it from a long sleep. $! is the PID of monado-service itself.
# The sleep never writes, so it would outlive monado-service by 68 years and keep any inherited descriptor (such as a
# flock without -o) open: it is killed explicitly below.
# A SIGKILLed service leaves its socket behind; with it the wait below would start the client too early.
rm -f "${XDG_RUNTIME_DIR:-/run/user/$UID}/monado_comp_ipc"
sleep 2147483647 | "$SVC" >"$LOGDIR/monado.log" 2>&1 &
SVC_PID=$!
SLEEP_PID=$(jobs -p | tail -1)   # process group leader of the pipeline = the sleep
stop_service() {
	if kill -INT "$SVC_PID" 2>/dev/null; then
		for _ in $(seq 75); do kill -0 "$SVC_PID" 2>/dev/null || break; sleep 0.2; done
		if kill -0 "$SVC_PID" 2>/dev/null; then
			echo "monado-service did not stop within 15 s, SIGKILL"
			kill -KILL "$SVC_PID" 2>/dev/null
		fi
	fi
	[ -n "$SLEEP_PID" ] && kill "$SLEEP_PID" 2>/dev/null
	return 0
}
trap 'exit 130' INT TERM
trap stop_service EXIT
for _ in $(seq 80); do
	[ -S "${XDG_RUNTIME_DIR:-/run/user/$UID}/monado_comp_ipc" ] && break
	kill -0 "$SVC_PID" 2>/dev/null || { tail -20 "$LOGDIR/monado.log"; die "monado-service exited at start-up"; }
	sleep 0.25
done
grep -E "pimax lens" "$LOGDIR/monado.log" | head -8
echo "--- starting pimax-lenstest ---"
"$APP" "${ARGS[@]}"
RC=$?
stop_service
trap - INT TERM
# NVIDIA display engine trouble after a start/stop shows up in the kernel log (see overnight-rules.md)
XID1=$(journalctl -k -b 2>/dev/null | grep -cE "Xid|Idling display engine")
if [ "$XID1" -gt "$XID0" ]; then
	journalctl -k -b 2>/dev/null | grep -E "Xid|Idling display engine" | tail -3
	echo "!!! the kernel log shows NEW NVIDIA display errors (above): do not start another headset run, reboot first"
fi
echo "client exit code $RC; monado log: $LOGDIR/monado.log"
exit "$RC"
