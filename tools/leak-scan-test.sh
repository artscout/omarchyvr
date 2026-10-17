#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Tests of the leak scanner's eye-chip allowance (tools/leak-scan.py, EYE_BASE_*) and of its Dream Air reference
# calibration allowance (CALIB_REFERENCE, checks 5-8 at the end), in a throw-away repo (a temp index of its own; your
# clone's index is never touched):
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

# ---- the Dream Air reference calibration (CALIB_REFERENCE, user decision 2026-10-17) ------------------------------
#   5. the shipped reference files pass (path + sha256 match)
#   6. a changed reference is refused, and so is the same content under another path
#   7. a synthetic fixture (fake deviceUID) is fine
#   8. the private originals with their serials (when this PC has them) are refused at the reference path and at their
#      own path; nothing of them is printed (the scanner prints kinds and paths only, the paths here carry no serial)
REF=rust/pimax-device/devices/dream-air/reference
FILES=(headset-calibration.xml controllers/left.json controllers/right.json lens.conf ctrl-cam-extrinsics.json)
stage_ref() {
    for f in "${FILES[@]}"; do
        mkdir -p "$T/$REF/$(dirname "$f")"
        cp "$REPO/$REF/$f" "$T/$REF/$f"
        git -C "$T" add -f "$REF/$f"
    done
}
stage_ref
expect "the shipped reference calibration is allowed" 0
stage_ref
echo "cant_deg = 3.0" >>"$T/$REF/lens.conf"
git -C "$T" add -f "$REF/lens.conf"
expect "a changed reference lens profile is refused" 1
stage_ref
sed -i 's/deviceUID="reference"/deviceUID="reference2"/' "$T/$REF/headset-calibration.xml"
git -C "$T" add -f "$REF/headset-calibration.xml"
expect "a changed reference calibration is refused" 1
mkdir -p "$T/docs"
for f in headset-calibration.xml controllers/left.json lens.conf ctrl-cam-extrinsics.json; do
    cp "$REPO/$REF/$f" "$T/docs/copy-$(basename "$f")"
    git -C "$T" add -f "docs/copy-$(basename "$f")"
    expect "the reference $f under another path is refused" 1
    rm -f "$T/docs/copy-$(basename "$f")"
done
mkdir -p "$T/rust/x/tests"
printf '<?xml version="1.0" ?>\n<DeviceConfiguration deviceUID="P511X0X000000000001" cameraModel="ov7251">\n</DeviceConfiguration>\n' >"$T/rust/x/tests/synthetic.xml"
git -C "$T" add -f rust/x/tests/synthetic.xml
expect "a synthetic fixture with a fake deviceUID is fine" 0
ORIG_XML="$HOME/pimax-re/analysis/headset-calibration.xml"
H1="$HOME/pimax-re/pimax-data/ProgramData-Pimax/slam/H1"
if [[ -f $ORIG_XML ]]; then
    stage_ref
    cp "$ORIG_XML" "$T/$REF/headset-calibration.xml"
    git -C "$T" add -f "$REF/headset-calibration.xml"
    expect "the private factory calibration at the reference path is refused" 1
    mkdir -p "$T/analysis"
    cp "$ORIG_XML" "$T/analysis/headset-calibration.xml"
    git -C "$T" add -f analysis/headset-calibration.xml
    expect "the private factory calibration at its own path is refused" 1
    rm -f "$T/analysis/headset-calibration.xml"
else
    echo "SKIP the private factory calibration checks: $ORIG_XML is not on this PC"
fi
first=$(ls "$H1" 2>/dev/null | head -1)
if [[ -n $first && -f $H1/$first/controller_params.json ]]; then
    mkdir -p "$T/pimax-data/ProgramData-Pimax/slam/H1/SERIAL" "$T/rust/x/data"
    cp "$H1/$first/controller_params.json" "$T/pimax-data/ProgramData-Pimax/slam/H1/SERIAL/controller_params.json"
    git -C "$T" add -f pimax-data/ProgramData-Pimax/slam/H1/SERIAL/controller_params.json
    expect "Pimax's controller models at their private path are refused" 1
    cp "$H1/$first/controller_params.json" "$T/rust/x/data/controller_params.json"
    git -C "$T" add -f rust/x/data/controller_params.json
    expect "Pimax's controller models under another path are refused" 1
else
    echo "SKIP the private controller model checks: no Windows data under $H1"
fi
[[ $fail -eq 0 ]] && echo "leak-scan eye and calibration allowances: all checks passed" || { echo "leak-scan allowances: FAILED"; exit 1; }
