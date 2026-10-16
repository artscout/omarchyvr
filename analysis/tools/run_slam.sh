#!/bin/bash
# 6DoF head tracking (Monado SLAM/Basalt) for the Dream Air. Design: analysis/research/6dof-head-tracking.md
#
#   run_slam.sh record [SECS]   EuRoC recording (cameras 0,1 + IMU), no tracker needed. Default 90 s.
#   run_slam.sh live   [SECS]   PIMAX_SLAM=1 + headless pose printer (position, "6dof" flag); also records.
#   run_slam.sh replay DIR      offline: check the recording, run Basalt on it (monado-cli slambatch, real time),
#                               print loop-closure drift and trajectory statistics next to the Windows pose log.
#                               DIR = a recording directory (or a EuRoC root).
#   run_slam.sh sweep DIR       replay with camera-IMU offsets -4..+4 ms (2 ms steps), loop drift per offset.
#
# Env: M (Monado build, default ~/.local/lib/omarchyvr/monado), PIMAX_SLAM_CAMS (default 0,1; try 0,1,2,3),
#      PIMAX_SLAM_CAM_OFFSET_US (live camera-IMU time offset), CAM_OFFSET_NS (replay: camera timestamps shifted
#      in a linked copy of the dataset; Basalt's own cam_time_offset_ns is ignored by its VIO),
#      PIMAX_CAM_AE (live: 0 off / dry / 1 send, see analysis/slam-exposure.md), WINLOG (Windows pose log for the
#      comparison), VIT_SYSTEM_LIBRARY_PATH (default ~/.local/lib/libbasalt.so).
set -u
MODE=${1:-}
M=${M:-$HOME/.local/lib/omarchyvr/monado}
T=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# The newest build that has every subcommand used here (an old $CARGO_TARGET_DIR binary had no "stats").
if [ -z "${SLAMTOOL:-}" ]; then
	for c in $HOME/src/target-slam/release/omarchyvr-slam $HOME/.local/bin/omarchyvr-slam; do
		[ -x "$c" ] && "$c" stats 2>&1 | grep -q "unknown command" && continue
		[ -x "$c" ] && { [ -z "${SLAMTOOL:-}" ] || [ "$c" -nt "$SLAMTOOL" ]; } && SLAMTOOL=$c
	done
	SLAMTOOL=${SLAMTOOL:-$HOME/.local/bin/omarchyvr-slam}
fi
export VIT_SYSTEM_LIBRARY_PATH=${VIT_SYSTEM_LIBRARY_PATH:-$HOME/.local/lib/libbasalt.so}
export PIMAX_LOG=${PIMAX_LOG:-info}
RECROOT=$HOME/.local/share/omarchyvr/slam-recordings
WINLOG=${WINLOG:-$T/../../pimax-data/windows-20261014/pimax-windows-dump-20261014-042415/pimax-poses.csv}

replay_one() { # $1 = recording dir, $2 = camera offset ns; prints the result dir
	local DIR=$1 OFF=${2:-0} CFG ROOT N SECS RES
	CFG=$DIR/basalt; mkdir -p "$CFG"
	# Gyro bias seed from the recording's first still second (what the live driver had cached), see slam-drift.md.
	"$SLAMTOOL" export --out "$CFG" ${PIMAX_SLAM_CAMS:+--cams $PIMAX_SLAM_CAMS} --bias-from "$DIR" >/dev/null || return 1
	ROOT=$(ls -d "$DIR"/euroc_* 2>/dev/null | tail -n 1); ROOT=${ROOT:-$DIR}
	if [ "$OFF" != 0 ]; then
		"$SLAMTOOL" shift "$ROOT" "$DIR/shift_$OFF" --cam-offset-ns "$OFF" >/dev/null || return 1
		ROOT=$DIR/shift_$OFF
	fi
	N=$(($(wc -l <"$ROOT/mav0/cam0/data.csv") - 1))
	SECS=$((N / 25 + 10))
	RES=$DIR/replay-$(date +%H%M%S)-off$OFF
	echo "*** Basalt on $ROOT ($N frame sets, ~${SECS}s real time, camera offset $OFF ns) -> $RES ***" >&2
	(sleep "$SECS"; echo) | EUROC_MAX_SPEED=false EUROC_PRINT_PROGRESS=false \
		timeout $((SECS + 30)) "$M/src/xrt/targets/cli/monado-cli" slambatch "$ROOT" "$CFG/pimax-basalt.toml" "$RES" \
		>"$RES.log" 2>&1
	echo "$RES"
}

service() { # $1 = seconds; runs monado-service in the background, log in $OUT
	export XR_RUNTIME_JSON=$M/openxr_monado-dev.json
	# Same compositor env as `omarchyvr vr start` (rust/omarchyvr-tools/src/vr/config.rs build_env). Without
	# XRT_COMPOSITOR_DESIRED_MODE and the connector the compositor picks another display mode and the headset
	# stays black even though the app is FOCUSED. Override any of them from the caller's environment.
	export XRT_COMPOSITOR_FORCE_WAYLAND_DIRECT=${XRT_COMPOSITOR_FORCE_WAYLAND_DIRECT:-1}
	export XRT_COMPOSITOR_WAYLAND_CONNECTOR=${XRT_COMPOSITOR_WAYLAND_CONNECTOR:-DP-1}
	export XRT_COMPOSITOR_FORCE_NVIDIA=${XRT_COMPOSITOR_FORCE_NVIDIA:-0}
	export XRT_COMPOSITOR_DESIRED_MODE=${XRT_COMPOSITOR_DESIRED_MODE:-2} PIMAX_FPS=${PIMAX_FPS:-72}
	export XRT_COMPOSITOR_LOG=${XRT_COMPOSITOR_LOG:-info} XRT_NO_STDIN=1 IPC_IGNORE_VERSION=1
	(sleep $(($1 + 15)) | "$M/src/xrt/targets/service/monado-service" >"$OUT/monado.log" 2>&1) &
}

stop_service() {
	pkill -x -INT monado-service
	sleep 3
	grep -E "EuRoC recording|6DoF|slam:|SLAM" "$OUT/monado.log" | tail -n 12
}

case "$MODE" in
record)
	SECS=${2:-90}
	OUT=$RECROOT/$(date +%Y%m%d-%H%M%S); mkdir -p "$OUT"
	export PIMAX_SLAM_RECORD=$OUT
	echo "*** Recording to $OUT for ${SECS}s ***"
	echo "1. Headset on the desk, still, for the first 5 s (gyro calibration + IMU noise window)."
	echo "2. Then pick it up (or wear it) in a LIT room: look around slowly, walk a loop of a few metres,"
	echo "   turn, look at near (0.5 m) and far things, and put it back EXACTLY where it started for the last 5 s."
	service "$SECS"
	sleep "$SECS"
	stop_service
	echo "next: $0 replay $OUT"
	;;
live)
	SECS=${2:-60}
	OUT=$RECROOT/$(date +%Y%m%d-%H%M%S)-live; mkdir -p "$OUT"
	export PIMAX_SLAM=1 PIMAX_SLAM_RECORD=$OUT
	[ -f "$VIT_SYSTEM_LIBRARY_PATH" ] || { echo "no $VIT_SYSTEM_LIBRARY_PATH (build Basalt first)"; exit 1; }
	# Same Basalt settings as the offline replay (calibration file + VIO config); unset SLAM_CONFIG=... to use
	# the calibration the driver hands over with Basalt's built-in defaults instead.
	"$SLAMTOOL" export --out "$OUT/basalt" ${PIMAX_SLAM_CAMS:+--cams $PIMAX_SLAM_CAMS} >/dev/null || exit 1
	export SLAM_CONFIG=${SLAM_CONFIG-$OUT/basalt/pimax-basalt.toml}
	[ "$T/posecli" -nt "$T/posecli.c" ] || gcc -O2 -o "$T/posecli" "$T/posecli.c" -lopenxr_loader -lm || exit 1
	echo "*** Live 6DoF for ${SECS}s, log in $OUT ***"
	echo "Headset still on the desk for 5 s, then move it by hand: positions must follow (p=...), lines end in '6dof'."
	service "$SECS"
	sleep "${POSE_DELAY:-4}" # run_slam_view.sh delays this so the camera view connects first and is the active app
	XR_RUNTIME_JSON=$M/openxr_monado-dev.json timeout $((SECS + 5)) "$T/posecli" "$SECS" | tee "$OUT/poses.txt" |
		awk 'NR % 5 == 0'
	stop_service
	echo "pose log $OUT/poses.txt; offline replay of the same data: $0 replay $OUT"
	;;
replay)
	DIR=${2:?recording directory}
	"$SLAMTOOL" check "$DIR" || echo "(check reported problems; continuing)"
	RES=$(replay_one "$DIR" "${CAM_OFFSET_NS:-0}") || exit 1
	wc -l "$RES"/tracking.csv 2>/dev/null || { echo "no poses; see $RES.log"; exit 1; }
	"$SLAMTOOL" loop "$RES/tracking.csv"
	# Raw Basalt output: the driver additionally holds the position while the headset lies still and resets a
	# tracker that drifts meanwhile (pimax-cam StillParams); this replay does not include that.
	if [ -f "$WINLOG" ]; then
		"$SLAMTOOL" stats "$RES/tracking.csv" --vs "$WINLOG" --vs-format pimax-windows
	else
		"$SLAMTOOL" stats "$RES/tracking.csv"
	fi
	;;
sweep)
	DIR=${2:?recording directory}
	for OFF in -4000000 -2000000 0 2000000 4000000; do
		RES=$(replay_one "$DIR" "$OFF") || continue
		printf '%+9d ns: ' "$OFF"; "$SLAMTOOL" loop "$RES/tracking.csv" 2>&1 | tail -n 1
	done
	echo "smallest end-start distance = best camera-IMU offset (live: PIMAX_SLAM_CAM_OFFSET_US = ns / 1000)"
	;;
*)
	sed -n '2,16p' "$0"
	exit 2
	;;
esac
