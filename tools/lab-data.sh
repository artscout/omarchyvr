#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# lab-data.sh: put the private lab data that some tests replay into this checkout, or take it out again.
#
#   tools/lab-data.sh link   [LAB_DIR]   link the private files into the tree (tests then build with cfg(lab_data))
#   tools/lab-data.sh unlink             remove the links and restore the public stand-ins
#   tools/lab-data.sh status             show what is linked
#
# LAB_DIR (default: $PIMAX_LAB_DATA, else <repo>/.claude/lab/repo) mirrors the repository paths of the private files:
# the factory calibration of the lab headset, the camera recording of the room, the eye-chip USB transcripts and
# the cold-start frame list. It is never tracked (.gitignore ignores .claude/ and every linked path).
# rust/lab_data_build.rs sets cfg(lab_data) when the files are present; without them those tests are ignored.
# Two files are tracked synthetic stand-ins in the public tree; while linked they are marked skip-worktree so git
# status stays clean, and `unlink` restores them from HEAD. A clean worktree used for an install build can be linked
# too: `tools/lab-data.sh link /path/to/.claude/lab/repo` (the eye frame list is then the real one).
set -euo pipefail
REPO="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
cd "$REPO"
PRIVATE=(
    analysis/headset-calibration.xml
    rust/pimax-cam/tests/fixtures/evening2.pxcam
    'rust/omarchyvr-passthrough/tests/fixtures/*.png'
    rust/fixtures/if0_replies_coldstart.rec.gz
    'rust/pimax-eye/fixtures/*.usbpcap.gz'
    'rust/pimax-eye/fixtures/*.golden.gz'
)
STANDINS=(
    rust/pimax-cam/tests/fixtures/headset-calibration.xml
    rust/pimax-eye/data/replay-frames.txt
)

cmd="${1:-status}"
LAB="$(realpath -m "${2:-${PIMAX_LAB_DATA:-$REPO/.claude/lab/repo}}")"

files() {
    local pat f
    for pat in "${PRIVATE[@]}"; do
        for f in "$LAB"/$pat; do
            [[ -f "$f" ]] && printf '%s\n' "${f#"$LAB"/}"
        done
    done
    return 0
}

case "$cmd" in
link)
    [[ -d "$LAB" ]] || { echo "no lab data at $LAB (tests that need it stay ignored)"; exit 0; }
    n=0
    while IFS= read -r rel; do
        if git ls-files --error-unmatch "$rel" >/dev/null 2>&1; then
            echo "refuse: $rel is tracked" >&2; exit 1
        fi
        mkdir -p "$(dirname "$rel")"
        ln -sfn "$LAB/$rel" "$rel"; n=$((n + 1))
    done < <(files)
    for rel in "${STANDINS[@]}"; do
        [[ -f "$LAB/$rel" ]] || continue
        git update-index --skip-worktree "$rel"
        ln -sfn "$LAB/$rel" "$rel"; n=$((n + 1))
    done
    echo "linked $n private files from $LAB"
    ;;
unlink)
    n=0
    for pat in "${PRIVATE[@]}"; do
        for f in $pat; do
            [[ -L "$f" ]] && { rm -f "$f"; n=$((n + 1)); }
        done
    done
    for rel in "${STANDINS[@]}"; do
        if [[ -L "$rel" ]]; then
            rm -f "$rel"; n=$((n + 1))
        fi
        git update-index --no-skip-worktree "$rel"
        git checkout -- "$rel"
    done
    echo "removed $n links, stand-ins restored"
    ;;
status)
    for pat in "${PRIVATE[@]}" "${STANDINS[@]}"; do
        for f in $pat; do
            if [[ -L "$f" ]]; then echo "linked  $f"; elif [[ -e "$f" ]]; then echo "public  $f"; else echo "absent  $f"; fi
        done
    done
    ;;
*)
    sed -n '4,17p' "$0"; exit 2 ;;
esac
