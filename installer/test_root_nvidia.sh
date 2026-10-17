#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Test of the NVIDIA parts of installer/omarchyvr-root.sh on scratch trees: no root, nothing under /usr/src, /etc or
# /var is written (the plan mode only reads).
#   1. `restore-stock` (the uninstall's "put the stock NVIDIA source back"): files with a *.pimax-orig backup, patches
#      applied by hand without a backup (the real-machine bug of 2026-10-17), a pristine reference copy, and a tree that
#      cannot be restored (must be reported, not hidden).
#   2. `plan` with a tested, an untested-refused and an untested-allowed driver.
# Needs the pristine NVIDIA files the patches touch: $NVIDIA_PRISTINE_DIR (default /usr/src/nvidia-610.57.04.orig).
# Exit 77 when they are not available.
set -u
export NVIDIA_PATCH_NO_SYSLOG=1
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/.." && pwd)
ROOT_SH=$HERE/omarchyvr-root.sh
PD=$REPO/patches/nvidia
PRISTINE=${NVIDIA_PRISTINE_DIR:-/usr/src/nvidia-610.57.04.orig}
fails=0
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=$((fails + 1)); }
T=$(mktemp -d "${TMPDIR:-/tmp}/rootnv-test.XXXXXX")
trap 'chmod -R u+w "$T" 2>/dev/null; rm -rf "$T"' EXIT
export NVIDIA_PATCH_CONF=$T/none.conf NVIDIA_PATCH_STATUS=$T/status/s

TOUCHED=$(cat "$PD"/nvidia-*.patch | sed -n 's|^+++ b/||p' | sort -u)
if [ ! -f "$PRISTINE/src/nvidia-modeset/src/nvkms-evo.c" ]; then
	echo "SKIP: no pristine NVIDIA source at $PRISTINE"
	exit 77
fi
P=$T/pristine
for f in $TOUCHED; do mkdir -p "$P/$(dirname "$f")"; cp -p "$PRISTINE/$f" "$P/$f"; done
[ -z "$(grep -rl 'merged_head_sync\|nvEvoReadChannelGetPut\|pb\.dead' "$P")" ] && ok "the pristine files carry no patch code" || { bad "the 'pristine' source already has patch code"; exit 1; }

mk() { rm -rf "$T/$1"; mkdir -p "$T/$1"; cp -a "$P" "$T/$1/nvidia-610.57.04"; echo "$T/$1/nvidia-610.57.04"; }
same_as_pristine() { # TREE: every touched file equals the pristine one
	local f
	for f in $TOUCHED; do cmp -s "$1/$f" "$P/$f" || { echo "differs: $f"; return 1; }; done
}
apply_all() { local p; for p in "$PD/nvidia-2h1or-viewport.patch" "$PD/nvidia-2h1or-flip-fix.patch" "$PD/nvidia-evo-makeroom-bounded-wait.patch"; do patch -p1 -s --no-backup-if-mismatch -d "$1" -i "$p" || return 1; done; }
restore() { # restore TREE [PATCHDIR]: run the restore, output in $T/out.txt
	"$ROOT_SH" restore-stock "$@" >"$T/out.txt" 2>&1
}

# 1a. patched by the re-apply script (backups next to the files): the backups are enough
t=$(mk a)
"$PD/nvidia-patch-reapply.sh" --src-root "$(dirname "$t")" >/dev/null 2>&1
[ -n "$(find "$t" -name '*.pimax-orig')" ] && ok "setup: patched with backups" || bad "setup a"
restore "$t" "$PD" && same_as_pristine "$t" && [ -z "$(find "$t" -name '*.pimax-orig')" ] && ok "backups restore the stock source" || { bad "1a: $(cat "$T/out.txt")"; }

# 1b. patched by hand, no backups at all
t=$(mk b)
apply_all "$t" && ! same_as_pristine "$t" >/dev/null && ok "setup: hand-patched, no backups" || bad "setup b"
restore "$t" "$PD" && same_as_pristine "$t" && ok "patches without backups are reversed" || bad "1b: $(cat "$T/out.txt")"
grep -q "reverse nvidia-2h1or-viewport.patch in src/nvidia-modeset/src/nvkms-evo.c" "$T/out.txt" && ok "says which patch it reversed" || bad "1b output"
grep -q "no patch code left" "$T/out.txt" && ok "reports the stock source" || bad "1b: no stock report"

# 1c. the real-machine case: backups for most files, three files patched without backups
t=$(mk c)
"$PD/nvidia-patch-reapply.sh" --src-root "$(dirname "$t")" >/dev/null 2>&1
for f in src/nvidia-modeset/include/nvkms-dma.h src/nvidia-modeset/src/nvkms-dma.c src/nvidia-modeset/src/nvkms-evo.c; do rm -f "$t/$f.pimax-orig"; done
restore "$t" "$PD"
rc=$?
if [ $rc = 0 ] && same_as_pristine "$t"; then ok "mixed: backups restored, the three backup-less files reversed"; else bad "1c (rc $rc): $(cat "$T/out.txt")"; fi
[ -z "$(find "$t" -name '*.pimax-orig')" ] && ok "mixed: no backups left" || bad "1c: backups left"

# 1d. a second run is a no-op
restore "$t" "$PD" && ! grep -q "reverse" "$T/out.txt" && ok "a stock tree is left alone" || bad "1d: $(cat "$T/out.txt")"

# 1e. only the viewport patch hand-applied
t=$(mk e)
patch -p1 -s -d "$t" -i "$PD/nvidia-2h1or-viewport.patch"
restore "$t" "$PD" && same_as_pristine "$t" && ok "a single hand-applied patch is reversed" || bad "1e: $(cat "$T/out.txt")"

# 1f. edited by hand around the patch so that -R fails: the pristine copy is the reference
t=$(mk f)
apply_all "$t"
sed -i 's|^\(    pDevEvo->hal->SetOutputScaler(pDispEvo, head, updateState);\)$|\1 /* mine */\n    /* mine too */|' "$t/src/nvidia-modeset/src/nvkms-evo.c"
sed -i 's|\(Specify safe default values for viewPortPointIn x and y; these may be\)|\1 (edited)|' "$t/src/nvidia-modeset/src/nvkms-evo.c"
cp -a "$P" "$t.orig"
restore "$t" "$PD"
if same_as_pristine "$t"; then ok "with a pristine .orig copy, files that cannot be reversed are copied back"; else bad "1f: $(cat "$T/out.txt")"; fi
grep -q "copy .* back from the pristine copy" "$T/out.txt" && ok "says it used the pristine copy" || bad "1f output"

# 1g. cannot be restored: no backup, no pristine copy, edited around the patch -> reported loudly, exit 1
t=$(mk g)
apply_all "$t"
sed -i 's|^\(    pDevEvo->hal->SetOutputScaler(pDispEvo, head, updateState);\)$|\1 /* mine */\n    /* mine too */|' "$t/src/nvidia-modeset/src/nvkms-evo.c"
sed -i 's|\(Specify safe default values for viewPortPointIn x and y; these may be\)|\1 (edited)|' "$t/src/nvidia-modeset/src/nvkms-evo.c"
restore "$t" "$PD"
rc=$?
[ $rc = 1 ] && grep -q "STILL CONTAINS PATCH CODE" "$T/out.txt" && ok "an unrestorable tree is reported (exit 1)" || bad "1g (rc $rc): $(cat "$T/out.txt")"
grep -q "per-head viewport offset" "$T/out.txt" && ok "names the marker that is left" || bad "1g marker name"
# the other patches were still reversed (nothing is left half way)
! grep -rqs 'merged_head_sync\|pb\.dead' "$t" && ok "the patches that could be reversed were" || bad "1g: other patches still in"

# 1h. a .orig copy that is itself patched is not trusted
t=$(mk h)
apply_all "$t"
cp -a "$t" "$t.orig"
restore "$t" "$PD"
same_as_pristine "$t" && ok "a patched .orig is not used, the patches are reversed instead" || bad "1h: $(cat "$T/out.txt")"
grep -q "not using .* as the reference" "$T/out.txt" && ok "says why" || bad "1h output"

# 2. the plan: tested / untested refused / untested allowed
DATA=$T/data
mkdir -p "$DATA"
ln -s "$REPO/linux" "$DATA/linux"
ln -s "$REPO/patches" "$DATA/patches"
ln -s "$REPO/installer" "$DATA/installer"
plan() { # plan VERSION ALLOW
	cat >"$T/plan.env" <<EOF
OVR_USER=nobody
OVR_HOME=$T/home
OVR_DATA=$DATA
OVR_NVIDIA=patch
OVR_NVIDIA_VERSION=$1
OVR_NVIDIA_TESTED=610.57.04
OVR_NVIDIA_ALLOW_UNTESTED=$2
OVR_SNAPSHOT=no
EOF
	"$ROOT_SH" plan "$T/plan.env" >"$T/out.txt" 2>&1
}
plan 610.57.04 no && grep -q "NVIDIA 610.57.04 (tested)" "$T/out.txt" && ! grep -q "WARNING" "$T/out.txt" && ok "plan: tested driver, no warning" || bad "plan tested: $(cat "$T/out.txt")"
grep -q "/etc/omarchyvr/nvidia.conf" "$T/out.txt" && ok "plan: writes the choice to nvidia.conf" || bad "plan: conf not listed"
plan 999.1 no && grep -q "NVIDIA 999.1 (NOT tested): the patch is NOT applied" "$T/out.txt" && grep -q "Allow the patch on untested NVIDIA drivers" "$T/out.txt" && ok "plan: untested, not allowed -> not applied, says how to allow" || bad "plan untested refused: $(cat "$T/out.txt")"
! grep -q "dkms build + install" "$T/out.txt" && ok "plan: untested refused builds nothing" || bad "plan untested refused would build"
plan 999.1 yes && grep -q "you allowed the patch on untested drivers" "$T/out.txt" && grep -q "WARNING: this driver version was never tested" "$T/out.txt" && ok "plan: untested + allowed shows the WARNING" || bad "plan untested allowed: $(cat "$T/out.txt")"
grep -q "restore snapshot\|stock module is built as a fallback" "$T/out.txt" && ok "plan: mentions the stock fallback" || bad "plan: no fallback text"
plan 999.1 maybe; [ $? = 0 ] && grep -q "the patch is NOT applied" "$T/out.txt" && ok "plan: anything but yes means no" || bad "plan: odd value"
printf 'OVR_NVIDIA_ALLOW_UNTESTED=yes; rm -rf /\n' >"$T/bad.env"
"$ROOT_SH" plan "$T/bad.env" >"$T/out.txt" 2>&1
[ $? -ne 0 ] && grep -q "unsafe value" "$T/out.txt" && ok "plan: an unsafe value is refused" || bad "plan: unsafe value accepted"

echo
[ "$fails" = 0 ] && echo "all checks passed" || echo "$fails check(s) FAILED"
exit $((fails > 0))
