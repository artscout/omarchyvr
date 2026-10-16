#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Run a long audit or bench only while no VR session runs, and stop it the moment one starts.
#   ai/scripts/guarded-audit.sh [--log FILE] -- COMMAND [ARGS...]
# Exit: 0/COMMAND's status, 3 = a session was running (not started), 4 = a session started (aborted).
set -u
LOG=/dev/stdout
if [[ "${1:-}" == --log ]]; then LOG="$2"; shift 2; fi
[[ "${1:-}" == -- ]] && shift
[[ $# -ge 1 ]] || { echo "usage: $0 [--log FILE] -- COMMAND [ARGS...]" >&2; exit 2; }
if pgrep -x monado-service >/dev/null; then echo "SESSION RUNNING: not started"; exit 3; fi
nice -n 19 "$@" > "$LOG" 2>&1 &
P=$!
while kill -0 "$P" 2>/dev/null; do
    if pgrep -x monado-service >/dev/null; then kill "$P"; echo "SESSION STARTED: aborted"; exit 4; fi
    sleep 2
done
wait "$P"
