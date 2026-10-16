#!/bin/bash
# A/B check of the lens model move to Rust: capture every test pattern with two Monado builds (the old C lens model and
# the new Rust one) and compare the compositor's raw final output pixel by pixel. Glue only; the work is done by
# run_lens_test.sh --capture and `pimax-lenstest dumpdiff`.
#
#   run_lens_ab.sh [OUTDIR]        default OUTDIR /tmp/lens-ab
# Environment:
#   OLD_BUILD  Monado build with the old C lens model + dump trigger (default ~/src/monado-lens-base/build:
#              git -C ~/src/monado worktree add ~/src/monado-lens-base lens-base, built like any Monado tree)
#   NEW_BUILD  Monado build of branch `lens` (default ~/src/monado-lens/build)
#   LENSTEST   the client (default rust/target/release/pimax-lenstest, then ~/src/target-lens/release)
#   NO_DEVICE  default 0 = the normal session (headset USB opened, panel on, like every working run). NO_DEVICE=1 does
#              not open the USB; the one overnight run with it dumped 2 frames and then stalled (see analysis/overnight/lens.md)
# Takes the headset lock once for both sessions (flock -o). Needs about 5 GB in OUTDIR (raw dumps, 109 MB each).
# Both sessions use the same lens.conf (~/.config/omarchyvr/lens.conf or --lens-conf FILE); with an old-format file the
# two outputs must be IDENTICAL. Exit code 0 only if every dump pair is identical.
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
if [ -z "${_LENS_LOCKED:-}" ]; then
	_LENS_LOCKED=1 exec flock -o -w 3600 /tmp/omarchyvr-headset.lock "$0" "$@"
fi
OUT=${1:-/tmp/lens-ab}
shift || true
OLD=${OLD_BUILD:-$HOME/src/monado-lens-base/build}
NEW=${NEW_BUILD:-$HOME/src/monado-lens/build}
APP=${LENSTEST:-}
if [ -z "$APP" ]; then
	for c in "$HERE/../../rust/target/release/pimax-lenstest" "$HOME/src/target-lens/release/pimax-lenstest"; do
		[ -x "$c" ] && { APP=$c; break; }
	done
fi
[ -x "$APP" ] || { echo "run_lens_ab: pimax-lenstest not built (cargo build --release -p pimax-lenstest), or set LENSTEST" >&2; exit 1; }
rm -rf "$OUT"; mkdir -p "$OUT"
for v in old new; do
	if [ $v = old ]; then B=$OLD; else B=$NEW; fi
	echo "=== capture with the $v build: $B ==="
	MONADO_BUILD=$B LENSTEST=$APP NO_DEVICE=${NO_DEVICE:-0} LOGDIR=$OUT/$v-log "$HERE/run_lens_test.sh" --capture "$OUT/$v" --keep-raw "$@" >"$OUT/$v-run.log" 2>&1
	echo "  exit $?, $(ls "$OUT/$v"/*.png 2>/dev/null | wc -l) pictures; log $OUT/$v-run.log"
	grep -E "CAPTURE FAILED|NEW NVIDIA" "$OUT/$v-run.log" | head -3
done
echo "=== raw dump comparison (old vs new) ==="
BAD=0
N=0
for f in "$OUT"/old/*.raw; do
	[ -e "$f" ] || { echo "no raw dumps from the old build"; exit 1; }
	b=$(basename "$f" .raw)
	stem=${b%%_frame*}   # 07-channels-red-only_frame123_7104x3840_fmt64 -> 07-channels-red-only
	g=$(ls "$OUT"/new/"$stem"_frame*.raw 2>/dev/null | head -1)
	N=$((N + 1))
	if [ -z "$g" ]; then echo "$stem: no dump in the new capture"; BAD=$((BAD + 1)); continue; fi
	if R=$("$APP" dumpdiff "$f" "$g" --png "$OUT/diff-$stem.png"); then
		echo "$stem: $R"; rm -f "$OUT/diff-$stem.png"
	else
		echo "$stem: $R"; BAD=$((BAD + 1))
	fi
done
echo "$N dump pairs compared, $BAD differ or are missing"
[ "$BAD" = 0 ] && [ "$N" -gt 0 ]
