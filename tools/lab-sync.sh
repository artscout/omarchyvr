#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# lab-sync.sh: bring the old lab repository's (pimax-re) changes between two lab commits into this repo, with the new names.
#
#   tools/lab-sync.sh [--check] OLD_LAB_COMMIT NEW_LAB_COMMIT LAB_DIR
#
# For each of the two lab commits: a shared clone of the lab at that commit runs ITS tools/export-public.sh --no-test
# (so scrubbing and branding are those of that commit), then tools/apply-names.py applies tools/names.tsv (old name ->
# omarchyvr name, paths and text) to the export. The difference of the two renamed exports is applied to this
# working tree with `git apply`. Only the lab's changes travel; edits made here stay unless the same lines changed.
#   --check   only `git apply --check` (nothing is changed); prints what would happen.
# On a conflict the hunks that apply are applied, the others are left as FILE.rej and listed. Commit the result yourself.
# Names and the rules behind the mapping: docs/NAMES.md.
set -euo pipefail

CHECK=0
if [[ ${1:-} == --check ]]; then CHECK=1; shift; fi
[[ $# -eq 3 ]] || { sed -n '3,15p' "$0" >&2; exit 2; }
OLD=$1 NEW=$2 LAB=$3
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
NAMES=$HERE/tools/names.tsv
TMP=$(mktemp -d "${TMPDIR:-/tmp}/lab-sync.XXXXXX")
trap 'rm -rf "$TMP"' EXIT

export_at() {   # commit, label
    local c=$1 l=$2
    git clone --shared -q "$LAB" "$TMP/lab-$l"
    git -C "$TMP/lab-$l" checkout -q "$c"
    nice -n 19 "$TMP/lab-$l/tools/export-public.sh" --no-test "$TMP/exp-$l" >"$TMP/export-$l.log" 2>&1 \
        || { echo "export of $c failed:"; tail -20 "$TMP/export-$l.log"; exit 1; }
    rm -rf "$TMP/exp-$l/.git"
    python3 -I "$HERE/tools/apply-names.py" "$TMP/exp-$l" --names "$NAMES"
}
export_at "$OLD" old
export_at "$NEW" new

cd "$TMP"
diff -ruN exp-old exp-new >"$TMP/sync.patch" || true   # exit 1 = differences
if [[ ! -s $TMP/sync.patch ]]; then echo "no differences between $OLD and $NEW"; exit 0; fi
echo "patch: $(grep -c '^+++ ' "$TMP/sync.patch") files, $(grep -c '^[+-][^+-]' "$TMP/sync.patch") changed lines"
cd "$HERE"
if git apply --check -p1 "$TMP/sync.patch" 2>"$TMP/check.err"; then
    echo "applies cleanly"
    [[ $CHECK -eq 1 ]] || { git apply -p1 "$TMP/sync.patch"; echo "applied; review with git status / git diff, then commit"; }
    exit 0
fi
# does it apply in reverse (the changes are already here)?
if git apply --check -R -p1 "$TMP/sync.patch" 2>/dev/null; then
    echo "already applied: the patch reverse-applies cleanly (this range was synced before)"
    exit 0
fi
# per file: applies / already applied / conflict
conflicts=0
for f in $(grep '^+++ ' "$TMP/sync.patch" | sed -e 's/^+++ exp-new\///' -e 's/\t.*//'); do
    if git apply --check -p1 --include="$f" "$TMP/sync.patch" 2>/dev/null; then echo "  would apply:     $f"
    elif git apply --check -R -p1 --include="$f" "$TMP/sync.patch" 2>/dev/null; then echo "  already applied: $f"
    else echo "  CONFLICT:        $f"; conflicts=1; fi
done
if [[ $CHECK -eq 0 ]]; then
    git apply --reject -p1 "$TMP/sync.patch" || true
    echo "hunks that applied are in the tree; the others are in *.rej:"; git ls-files --others --exclude-standard | grep '\.rej$' || true
    exit 1
fi
exit $conflicts
