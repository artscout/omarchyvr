#!/usr/bin/env bash
# Tests of tools/vr-busy.sh (the "no builds or tests during a VR session" guard) with a stand-in VR process: a copy
# of `sleep` under its own name, so the real session processes are never looked at or touched.
set -u
REPO=$(cd "$(dirname "$0")/.." && pwd)
G="$REPO/tools/vr-busy.sh"
T=$(mktemp -d)
trap 'pkill -P "$PID" 2>/dev/null; kill "$PID" 2>/dev/null; rm -rf "$T"' EXIT
fail=0
expect() { # expect NAME WANT GOT
	if [[ $2 == "$3" ]]; then echo "ok   $1"; else echo "FAIL $1 (want $2, got $3)"; fail=1; fi
}
FAKE=vrbusy-fake-$$
FAKE=${FAKE:0:15}
cp "$(command -v sleep)" "$T/$FAKE"
export OMARCHYVR_VR_BUSY_PROCS="$FAKE"
# the stand-in runs from a temporary dir: for the session cases it must count, so no test roots here
export OMARCHYVR_VR_BUSY_TEST_ROOTS="/nonexistent-test-root/"
unset OMARCHYVR_ALLOW_DURING_VR
PID=""

"$G" >/dev/null; expect "no session: free" 0 $?
"$G" --guard true; expect "no session: --guard runs the command" 0 $?
out=$("$G" --guard sh -c 'nice'); expect "the command runs at nice 19" 19 "$out"
"$G" --wait 5 >/dev/null; expect "--wait returns at once when free" 0 $?

"$T/$FAKE" 300 &
PID=$!
for _ in 1 2 3 4 5 6 7 8 9 10; do pgrep -x "$FAKE" >/dev/null && break; sleep 0.1; done
out=$("$G"); rc=$?
expect "session: busy" 1 $rc
[[ $out == *"$FAKE (pid $PID)"* ]]; expect "says which process runs" 0 $?
"$G" --guard touch "$T/ran" 2>"$T/err"; expect "session: --guard refuses (75)" 75 $?
[[ ! -e $T/ran ]]; expect "the refused command did not run" 0 $?
grep -q "a VR session is running" "$T/err"; expect "the refusal says why" 0 $?
OMARCHYVR_ALLOW_DURING_VR=1 "$G" --guard touch "$T/ran"; expect "the user's override runs it" 0 $?
OMARCHYVR_VR_BUSY_POLL_S=1 "$G" --wait 1 2>/dev/null; expect "--wait gives up after MAX_S" 1 $?
"$REPO/tools/vr-guard.sh" true 2>/dev/null; expect "the rustc-wrapper form refuses too" 75 $?
grep -q 'rustc-wrapper = "tools/vr-guard.sh"' "$REPO/.cargo/config.toml"; expect "cargo uses the guard as rustc-wrapper" 0 $?
grep -q 'runner = "tools/vr-busy.sh --guard"' "$REPO/.cargo/config.toml"; expect "cargo uses the guard as runner" 0 $?

kill "$PID" 2>/dev/null; wait "$PID" 2>/dev/null

# the Rust tests' stand-ins (fake monado-service under /tmp/omarchyvr-vr-test-<pid>-..., a binary or a script) are not
# a session with the default test roots
unset OMARCHYVR_VR_BUSY_TEST_ROOTS
mkdir -p "$T/omarchyvr-vr-test-1-x/bin"
cp "$(command -v sleep)" "$T/omarchyvr-vr-test-1-x/bin/$FAKE"
"$T/omarchyvr-vr-test-1-x/bin/$FAKE" 300 &
PID=$!
for _ in 1 2 3 4 5 6 7 8 9 10; do pgrep -x "$FAKE" >/dev/null && break; sleep 0.1; done
"$G" >/dev/null; expect "a test stand-in binary is not a session" 0 $?
"$G" --guard true; expect "--guard runs next to a test stand-in" 0 $?
kill "$PID" 2>/dev/null; wait "$PID" 2>/dev/null
# a script stand-in: comm is the script's name, /proc/PID/exe is bash, argv[1] is the script under the test root
printf '#!/bin/bash\nsleep 300\n' >"$T/omarchyvr-vr-test-1-x/bin/$FAKE"
chmod +x "$T/omarchyvr-vr-test-1-x/bin/$FAKE"
"$T/omarchyvr-vr-test-1-x/bin/$FAKE" &
PID=$!
for _ in 1 2 3 4 5 6 7 8 9 10; do pgrep -x "$FAKE" >/dev/null && break; sleep 0.1; done
pgrep -x "$FAKE" >/dev/null; expect "the script stand-in runs under its own name" 0 $?
"$G" >/dev/null; expect "a test stand-in script is not a session" 0 $?
# the same stand-in counts when its root is not a test root (a real session is never filtered by name)
OMARCHYVR_VR_BUSY_TEST_ROOTS="/nonexistent-test-root/" "$G" >/dev/null; expect "outside a test root it is a session" 1 $?
pkill -P "$PID" 2>/dev/null; kill "$PID" 2>/dev/null; wait "$PID" 2>/dev/null
OMARCHYVR_VR_BUSY_POLL_S=1 "$G" --wait 10 >/dev/null; expect "--wait returns once the session ended" 0 $?
exit $fail
