#!/bin/bash
# Eye tracking accuracy test: starts the EYE build of Monado (PIMAX_EYE=1) and runs the lens test client's `gaze` pattern
# (rust/pimax-lenstest, see its --help and analysis/eye-monado.md). Controllers are off so the keys go to THIS terminal.
#
# What you do: put the headset on, keep your head still, press SPACE here. First 5 dots (axis check: the script prints
# which PIMAX_EYE_FLIP_X/Y/Z to set if an axis is mirrored), then SPACE again for the 17-dot accuracy grid: look at each
# dot until it jumps to the next (0.6 s settle, 1 s measured). Then a live ring shows where the tracker thinks you look
# (grey = raw, green = with the fitted correction).
#
# Keys (this terminal):  space or a  start / next      r  restart      y  accept: write ~/.config/pimax/eye-correction.toml
#                        (backup of an existing file kept)      q  quit
# Results: printed, and saved as JSON (path printed; --gaze-out PATH|DIR changes it). Options of the client go through:
#   --gaze-points 13|17   --gaze-out PATH   --seconds S
#
# The service runs with PIMAX_EYE_CORRECTION=none by default so that the RAW tracker is measured (a correction file in
# use would be measured on top of itself). To check an existing correction, set PIMAX_EYE_CORRECTION=<file> yourself.
# The correction is read by the service (libpimax_capi) when it starts, so the Monado eye build must be relinked after
# `cargo build --release -p pimax-capi` once, and monado-service restarted; relinking drops its file capability:
#   ninja -C ~/src/monado-eye/build && sudo setcap cap_sys_nice+ep ~/src/monado-eye/build/src/xrt/targets/service/monado-service
#
# Environment: MONADO_BUILD (default ~/src/monado-eye/build), PIMAX_EYE_FLIP_X/Y/Z (service AND client read them),
# everything run_lens_test.sh understands (HEADSET_LOCK=1, MODE, SCALE, ...).
cd "$(dirname "$0")/../.." || exit 1
export MONADO_BUILD=${MONADO_BUILD:-$HOME/src/monado-eye/build}
export PIMAX_EYE=1
export PIMAX_EYE_CORRECTION=${PIMAX_EYE_CORRECTION:-none}
export PIMAX_CTRL_HANDS=0
SVC=$MONADO_BUILD/src/xrt/targets/service/monado-service
if command -v getcap >/dev/null && [ -x "$SVC" ] && ! getcap "$SVC" | grep -q cap_sys_nice; then
	echo "eye_accuracy: warning: $SVC has no cap_sys_nice (it is dropped when ninja relinks it)." >&2
	echo "              restore it with: sudo setcap cap_sys_nice+ep $SVC" >&2
fi
exec analysis/tools/run_lens_test.sh --patterns gaze "$@"
