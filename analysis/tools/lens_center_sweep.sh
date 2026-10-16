#!/bin/bash
# Lens centre sweep: shows the grid pattern (red optical-axis cross per eye) once per horizontal lens centre, so the
# wearer can pick the value where the two red crosses merge. Variants are ~/.config/omarchyvr/lens-ipd/c<value>.conf.
#   analysis/tools/lens_center_sweep.sh [values...]     default: 0.0071 0.0171 0.0271 0.0371 0.0471
cd "$(dirname "$0")/../.." || exit 1
VALUES=("$@")
[ ${#VALUES[@]} -eq 0 ] && VALUES=(0.0071 0.0171 0.0271 0.0371 0.0471)
for c in "${VALUES[@]}"; do
	f=$HOME/.config/omarchyvr/lens-ipd/c$c.conf
	[ -f "$f" ] || { echo "no $f"; continue; }
	echo "=== center $c ==="
	PIMAX_CTRL_HANDS=0 analysis/tools/run_lens_test.sh --lens-conf "$f" --patterns grid
	read -r -p "center $c: one red cross or two? (Enter for the next value) "
done
