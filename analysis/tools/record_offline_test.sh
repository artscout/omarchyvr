#!/bin/bash
# Offline test of the whole recording path WITHOUT a headset, a window or a display: Monado's recorder tap
# (comp_recorder.c) -> shared memory -> pimax-record -> ffmpeg -> mp4. Run it BETWEEN sessions (it refuses while a
# monado-service or wayvr runs). Glue only, modelled on run_hands_composite.sh.
#
#   monado-service    the build with the tap (MONADO_BUILD, default ~/src/monado-eye/build-rec) on the debug-image
#                     target (WAYLAND_DISPLAY points nowhere, DISPLAY unset), PIMAX_NO_DEVICE=1, scripted hands
#   pimax-passthrough the OpenXR app (a .pxcam replay), so the compositor squashes a projection layer
#   pimax-record run  --encoder ${ENCODER:-cpu} (cpu by default: no NVENC needed), SECS seconds, then SIGINT
#
#   record_offline_test.sh               -> $OUT/out/pimax-*.mp4, checked with ffprobe (both: EYES=both)
# Environment: MONADO_BUILD, PASSTHROUGH, RECORD (pimax-record binary), PXCAM, OUT, SECS (5), EYES (left), FPS (30),
#   ENCODER (cpu | nvenc | auto).
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
M=${MONADO_BUILD:-$HOME/src/monado-eye/build-rec}
SVC=$M/src/xrt/targets/service/monado-service
RT=$M/openxr_monado-dev.json
PT=${PASSTHROUGH:-$HERE/../../rust/target/release/pimax-passthrough}
REC=${RECORD:-$HERE/../../rust/target/release/pimax-record}
PXCAM=${PXCAM:-$HERE/../../rust/pimax-cam/tests/fixtures/evening2.pxcam}
OUT=${OUT:-/tmp/pimax-record-offline}
SECS=${SECS:-5}
EYES=${EYES:-left}
FPS=${FPS:-30}
ENCODER=${ENCODER:-cpu}

die() { echo "record_offline_test: $*" >&2; exit 1; }
for f in "$SVC" "$RT" "$PT" "$REC" "$PXCAM"; do [ -e "$f" ] || die "missing $f"; done
command -v ffprobe >/dev/null || die "ffprobe missing"
pgrep -x monado-service >/dev/null && die "a monado-service is running (a real session?): not touching it"
pgrep -x wayvr >/dev/null && die "a wayvr is running: not touching it"

rm -rf "$OUT"; mkdir -p "$OUT/run" "$OUT/session" "$OUT/out" "$OUT/cfg" "$OUT/pimaxd"
chmod 700 "$OUT/run"
# an isolated runtime dir: its own IPC socket and its own ring, nothing of a real session is visible
export XDG_RUNTIME_DIR=$OUT/run PIMAX_RECORD_SHM=$OUT/run/pimax-record.shm

PIDS=()
cleanup() {
	for ((i = ${#PIDS[@]} - 1; i >= 0; i--)); do
		p=${PIDS[$i]}
		kill -INT "$p" 2>/dev/null || continue
		for _ in $(seq 150); do kill -0 "$p" 2>/dev/null || break; sleep 0.1; done
		kill -0 "$p" 2>/dev/null && { echo "pid $p did not stop in 15 s, SIGTERM"; kill -TERM "$p" 2>/dev/null; }
	done
}
trap cleanup EXIT
trap 'exit 130' INT TERM

env -u DISPLAY WAYLAND_DISPLAY=pimax-record-offline-none XRT_COMPOSITOR_DISABLE_DEFERRED=1 \
	PIMAX_NO_DEVICE=1 PIMAX_CTRL_HANDS=1 PIMAX_CTRL_HANDS_DEMO=1 PIMAX_EYE=0 XRT_NO_STDIN=1 \
	XRT_LOG=${XRT_LOG:-info} PIMAX_LOG=${PIMAX_LOG:-info} \
	"$SVC" </dev/null >"$OUT/monado.log" 2>&1 &
PIDS+=($!)
for _ in $(seq 80); do
	[ -S "$XDG_RUNTIME_DIR/monado_comp_ipc" ] && break
	kill -0 "${PIDS[0]}" 2>/dev/null || { tail -20 "$OUT/monado.log"; die "monado-service exited at start-up"; }
	sleep 0.25
done
for _ in $(seq 60); do grep -q "Debug image target used" "$OUT/monado.log" && break; sleep 0.25; done
grep -q "Debug image target used" "$OUT/monado.log" || die "not on the debug-image target: stopping"
grep -q "recorder: tap ready" "$OUT/monado.log" || die "this monado-service has no recorder tap (MONADO_BUILD=$M)"

XR_RUNTIME_JSON=$RT IPC_IGNORE_VERSION=1 "$PT" run --source "$PXCAM" >"$OUT/passthrough.log" 2>&1 </dev/null &
PIDS+=($!)
sleep 3

PIMAX_VR_SESSION_DIR=$OUT/session XDG_CONFIG_HOME=$OUT/cfg PIMAXD_DIR=$OUT/pimaxd \
	PIMAX_RECORD_NO_MONADO_CHECK=1 PIMAX_RECORD_NO_NOTIFY=1 \
	"$REC" run --eyes "$EYES" --fps "$FPS" --encoder "$ENCODER" --dir "$OUT/out" >"$OUT/record.log" 2>&1 </dev/null &
RP=$!
sleep "$SECS"
kill -INT "$RP"
for _ in $(seq 300); do kill -0 "$RP" 2>/dev/null || break; sleep 0.1; done
kill -0 "$RP" 2>/dev/null && die "pimax-record did not finish within 30 s"
cleanup; PIDS=(); trap - EXIT

grep -E "recorder: (attached|detaching)" "$OUT/monado.log" | sed 's/\x1b\[[0-9;]*m//g'
cat "$OUT/session/record.status"
F=$(ls "$OUT"/out/*.mp4 2>/dev/null | head -1)
[ -n "$F" ] || die "no mp4 (see $OUT/record.log and $OUT/monado.log)"
ffprobe -v error -select_streams v:0 -count_frames -show_entries stream=codec_name,width,height,nb_read_frames:format=duration -of default=nw=1 "$F"
ffmpeg -v error -y -ss 1 -i "$F" -frames:v 1 "$OUT/frame.png" && echo "one frame: $OUT/frame.png"
echo "ok: $F"
