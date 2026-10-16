#!/usr/bin/env bash
# Voice command test on the desktop (no VR session needed): mic listener + command runner, in one terminal.
#   tools/voice-test.sh            real actions (approved Omarchy commands run; VR ones fail without a desk session)
#   tools/voice-test.sh --dry-run  only print what would run
# Say "Max, help" / "Max, volume up" / "Max, lock the screen" -> "yes". Ctrl-C stops both.
set -u
BIN=~/pimax-re/rust/target/release/omarchyvr-voice
LOG=${XDG_RUNTIME_DIR:-/tmp}/omarchyvr-voice-test.log
"$BIN" ping >/dev/null 2>&1 && { echo "a voice listener is already running (desk --voice?); stop it first"; exit 1; }

"$BIN" listen >"$LOG" 2>&1 &
LPID=$!
trap 'kill -INT $LPID 2>/dev/null; wait $LPID 2>/dev/null; echo; echo "stopped (listener log: $LOG)"' EXIT
for _ in $(seq 40); do "$BIN" ping >/dev/null 2>&1 && break; sleep 0.25; done
"$BIN" ping >/dev/null 2>&1 || { echo "listener did not start:"; tail -5 "$LOG"; exit 1; }

echo "== listening. Say: \"Max, help\", \"Max, volume up\", \"Max, next workspace\" (heard text below). Ctrl-C to stop."
"$BIN" tail &
TPID=$!
trap 'kill $TPID 2>/dev/null; kill -INT $LPID 2>/dev/null; wait $LPID 2>/dev/null; echo; echo "stopped (listener log: $LOG)"' EXIT
"$BIN" act --no-toast "$@"
