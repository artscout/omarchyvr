#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Test of tools/lab-data.sh link in a scratch git repository (never this checkout): a second `link` must leave links
# that already point at the lab copy untouched (same link, same directory mtime), so cargo's rerun-if-changed does not
# rebuild the crates Monado links and ./install.sh does not relink monado-service (which drops its cap_sys_nice;
# real install 2026-10-17 13:21). A link to another place is still corrected.
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
fails=0
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=$((fails + 1)); }
T=$(mktemp -d "${TMPDIR:-/tmp}/lab-data-test.XXXXXX")
trap 'rm -rf "$T"' EXIT

R=$T/repo LAB=$T/lab
mkdir -p "$R/tools" "$R/rust/pimax-cam/tests/fixtures" "$R/rust/pimax-eye/data" "$R/analysis"
cp "$HERE/lab-data.sh" "$R/tools/lab-data.sh"
echo public >"$R/rust/pimax-cam/tests/fixtures/headset-calibration.xml"
echo public >"$R/rust/pimax-eye/data/replay-frames.txt"
git -C "$R" init -q
git -C "$R" -c user.email=t@example.invalid -c user.name=t add -A
git -C "$R" -c user.email=t@example.invalid -c user.name=t commit -qm init
for f in analysis/headset-calibration.xml rust/pimax-cam/tests/fixtures/evening2.pxcam rust/pimax-cam/tests/fixtures/headset-calibration.xml rust/pimax-eye/data/replay-frames.txt; do
	mkdir -p "$LAB/$(dirname "$f")"
	echo private >"$LAB/$f"
done

out=$("$R/tools/lab-data.sh" link "$LAB" 2>&1)
if [[ -L $R/analysis/headset-calibration.xml && $(readlink "$R/analysis/headset-calibration.xml") == "$LAB/analysis/headset-calibration.xml" ]]; then
	ok "first link: $out"
else bad "first link did not link: $out"; fi
lm() { stat -c %Y.%y "$1"; } # lstat: the link's own time, or the directory's
before_link=$(lm "$R/analysis/headset-calibration.xml")
before_dir=$(lm "$R/analysis")
before_standin=$(lm "$R/rust/pimax-eye/data/replay-frames.txt")
sleep 1.1
out=$("$R/tools/lab-data.sh" link "$LAB" 2>&1)
if [[ $(lm "$R/analysis/headset-calibration.xml") == "$before_link" && $(lm "$R/analysis") == "$before_dir" && $(lm "$R/rust/pimax-eye/data/replay-frames.txt") == "$before_standin" ]]; then
	ok "second link leaves unchanged links alone (no mtime change): $out"
else bad "second link touched the links or the directory: $out"; fi
if echo "$out" | grep -q "already linked, left alone"; then ok "says how many were left alone"; else bad "no 'left alone' note: $out"; fi

# a link that points somewhere else is corrected
ln -sfn /nonexistent "$R/analysis/headset-calibration.xml"
"$R/tools/lab-data.sh" link "$LAB" >/dev/null 2>&1
if [[ $(readlink "$R/analysis/headset-calibration.xml") == "$LAB/analysis/headset-calibration.xml" ]]; then ok "a wrong link is corrected"; else bad "wrong link stayed: $(readlink "$R/analysis/headset-calibration.xml")"; fi

# unlink still restores the public stand-ins
"$R/tools/lab-data.sh" unlink >/dev/null 2>&1
if [[ ! -L $R/rust/pimax-eye/data/replay-frames.txt && $(cat "$R/rust/pimax-eye/data/replay-frames.txt") == public && ! -e $R/analysis/headset-calibration.xml ]]; then
	ok "unlink restores the stand-ins and removes the links"
else bad "unlink"; fi

echo
if [ "$fails" = 0 ]; then echo "all checks passed"; else echo "$fails check(s) FAILED"; fi
[ "$fails" = 0 ]
