#!/bin/bash
# Eye-tracking calibration + accuracy test (rust/pimax-eyecal): starts the EYE build of Monado (PIMAX_EYE=1, so the
# end-to-end check through XR_EXT_eye_gaze_interaction works) and runs pimax-eyecal. Glue only.
#
# What you do: headset on, keep your HEAD STILL, press A (or space here). Follow each dot with your EYES only:
# 9 calibration dots (within ±15 deg / ±12 deg), then 10 different validation dots. If the head moves, the dot turns
# amber and starts again. Results appear in the headset and here; then:
#   A        live cursor (green = calibrated, grey = raw)
#   menu / Y save to ~/.config/omarchyvr/eye-calibration.toml (old file kept as .bak-<utc>)
#   S        check through the runtime (what WayVR and other OpenXR apps get; the service applies the saved file)
#   P        smooth pursuit (a moving dot)      B / R  start over      Q  quit
#
#   eye_calibrate.sh [--points 13] [--pursuit] [--recenter] [other pimax-eyecal options, see pimax-eyecal --help]
#
# Environment: MONADO_BUILD (default ~/src/monado-eye/build), MODE (default 2), LOGDIR (default
# /tmp/pimax-eyecal), HEADSET_LOCK=1 (take /tmp/omarchyvr-headset.lock for the run).
# Relinking the Monado eye build drops its file capability; restore it with:
#   sudo setcap cap_sys_nice+ep ~/src/monado-eye/build/src/xrt/targets/service/monado-service
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
if [ "${HEADSET_LOCK:-0}" = 1 ] && [ -z "${_EYECAL_LOCKED:-}" ]; then
	_EYECAL_LOCKED=1 exec flock -o -w 3600 /tmp/omarchyvr-headset.lock "$0" "$@"
fi
M=${MONADO_BUILD:-$HOME/src/monado-eye/build}
SVC=$M/src/xrt/targets/service/monado-service
RT=$M/openxr_monado-dev.json
APP=${EYECAL:-$HERE/../../rust/target/release/pimax-eyecal}
LOGDIR=${LOGDIR:-/tmp/pimax-eyecal}
mkdir -p "$LOGDIR"
die() { echo "eye_calibrate: $*" >&2; exit 1; }
[ -x "$SVC" ] || die "no monado-service at $SVC (set MONADO_BUILD)"
[ -x "$APP" ] || die "pimax-eyecal not built: (cd ~/pimax-re/rust && cargo build --release -p pimax-eyecal)"
pgrep -x monado-service >/dev/null && die "a monado-service is already running; stop it first (omarchyvr vr stop / desk stop)"
if command -v getcap >/dev/null && ! getcap "$SVC" | grep -q cap_sys_nice; then
	echo "eye_calibrate: warning: $SVC has no cap_sys_nice (dropped by the relink); restore it with:" >&2
	echo "  sudo setcap cap_sys_nice+ep $SVC" >&2
fi

export XR_RUNTIME_JSON=$RT IPC_IGNORE_VERSION=1
export PIMAX_EYE=1 PIMAX_EYE_FLIP_X=${PIMAX_EYE_FLIP_X:-1} PIMAX_CTRL_HANDS=0
export PIMAX_LOG=${PIMAX_LOG:-info} XRT_LOG=${XRT_LOG:-info}
export XRT_COMPOSITOR_FORCE_WAYLAND_DIRECT=1 XRT_COMPOSITOR_WAYLAND_CONNECTOR=${CONNECTOR:-DP-1}
export XRT_COMPOSITOR_FORCE_NVIDIA=0 XRT_COMPOSITOR_DESIRED_MODE=${MODE:-2}

XID0=$(journalctl -k -b 2>/dev/null | grep -cE "Xid|Idling display engine")
rm -f "${XDG_RUNTIME_DIR:-/run/user/$UID}/monado_comp_ipc"
sleep 2147483647 | "$SVC" >"$LOGDIR/monado.log" 2>&1 &
SVC_PID=$!
SLEEP_PID=$(jobs -p | tail -1)
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
echo "monado log: $LOGDIR/monado.log   (grep pimax-eye for the runtime's eye session)"
"$APP" "$@"
RC=$?
stop_service
trap - INT TERM
XID1=$(journalctl -k -b 2>/dev/null | grep -cE "Xid|Idling display engine")
if [ "$XID1" -gt "$XID0" ]; then
	journalctl -k -b 2>/dev/null | grep -E "Xid|Idling display engine" | tail -3
	echo "!!! NEW NVIDIA display errors in the kernel log: reboot before the next headset run"
fi
grep -E "pimax-eye" "$LOGDIR/monado.log" | tail -5
echo "exit code $RC; results in ~/.local/share/omarchyvr/eye-calibration/"
exit "$RC"
