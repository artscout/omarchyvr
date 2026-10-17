#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Tests of the leak scanner's eye-chip allowance (tools/leak-scan.py, EYE_BASE_*), in a throw-away repo (a temp index
# of its own; your clone's index is never touched):
#   1. the base eye start list passes (path + sha256 match)
#   2. the same list with one added line is refused (hash differs)
#   3. the same bytes under another path are refused
#   4. the private full list (when the lab data is present) is refused, at its own path and at the base path
# Checks 2-4 need the private list to compare against: they are skipped, loudly, when it is not present.
# Usage: tools/leak-scan-test.sh [PRIVATE_LIST]   (default: <repo>/.claude/lab/repo/rust/pimax-eye/data/replay-frames.txt,
# else the main checkout's when this is a git worktree)
set -euo pipefail
REPO="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
BASE=rust/pimax-eye/data/base-frames.txt
PRIV="${1:-}"
if [[ -z "$PRIV" ]]; then
    common="$(git -C "$REPO" rev-parse --path-format=absolute --git-common-dir)"
    for d in "$REPO" "$(dirname "$common")"; do
        [[ -f "$d/.claude/lab/repo/rust/pimax-eye/data/replay-frames.txt" ]] && { PRIV="$d/.claude/lab/repo/rust/pimax-eye/data/replay-frames.txt"; break; }
    done
fi
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
git -C "$T" init -q
git -C "$T" config user.email t@example.invalid
git -C "$T" config user.name t
mkdir -p "$T/tools" "$T/$(dirname "$BASE")" "$T/.claude/lab/repo/$(dirname "$BASE")"
cp "$REPO/tools/leak-scan.py" "$T/tools/"
[[ -n "$PRIV" ]] && cp "$PRIV" "$T/.claude/lab/repo/rust/pimax-eye/data/replay-frames.txt"
fail=0
scan() { (cd "$T" && python3 -I tools/leak-scan.py --staged >"$T/out.txt" 2>&1; echo $? >"$T/rc"); cat "$T/rc"; }
expect() { # name expected_rc
    local rc; rc="$(scan)"
    if [[ "$rc" == "$2" ]]; then echo "ok   $1 (exit $rc)"; else echo "FAIL $1: exit $rc, wanted $2"; sed 's/^/     /' "$T/out.txt"; fail=1; fi
    git -C "$T" reset -q
}

cp "$REPO/$BASE" "$T/$BASE"
git -C "$T" add -f "$BASE"
expect "base list is allowed" 0

if [[ -z "$PRIV" ]]; then
    echo "SKIP checks 2-4: no private list to compare against"
else
    echo "# changed" >>"$T/$BASE"
    git -C "$T" add -f "$BASE"
    expect "base list with a changed byte is refused" 1
    cp "$REPO/$BASE" "$T/$BASE"
    mkdir -p "$T/docs"
    cp "$REPO/$BASE" "$T/docs/copy.txt"
    git -C "$T" add -f docs/copy.txt
    expect "same bytes under another path are refused" 1
    rm -f "$T/docs/copy.txt"
    mkdir -p "$T/rust/pimax-eye/data"
    cp "$PRIV" "$T/rust/pimax-eye/data/replay-frames.txt"
    git -C "$T" add -f rust/pimax-eye/data/replay-frames.txt
    expect "the private full list at its path is refused" 1
    git -C "$T" rm -q --cached -f rust/pimax-eye/data/replay-frames.txt 2>/dev/null || true
    cp "$PRIV" "$T/$BASE"
    git -C "$T" add -f "$BASE"
    expect "the private full list at the base path is refused" 1
fi
[[ $fail -eq 0 ]] && echo "leak-scan eye allowance: all checks passed" || { echo "leak-scan eye allowance: FAILED"; exit 1; }
