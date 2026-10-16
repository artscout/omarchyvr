#!/bin/bash
# Test of linux/nvidia-patch-reapply.sh on scratch copies (no root, nothing under /usr/src is touched, the real
# dkms / limine-mkinitcpio / mkinitcpio are never called: the script's command overrides point at recording shims).
# Needs a pristine nvkms-evo.c: $NVIDIA_PRISTINE_DIR (default /usr/src/nvidia-610.57.04.orig, the backup made when the
# patch was first installed); without it the pristine file is derived by reverse-applying the patch to a copy of the
# installed tree ($NVIDIA_INSTALLED_DIR, default /usr/src/nvidia-610.57.04). Exit 77 if neither is available.
#   linux/test_nvidia_patch_reapply.sh
set -u
export NVIDIA_PATCH_NO_SYSLOG=1 # the conflict tests must not write to the real journal
HERE=$(cd "$(dirname "$0")" && pwd)
SCRIPT=$HERE/nvidia-patch-reapply.sh
PATCH=$HERE/nvidia-2h1or-viewport.patch
PRISTINE=${NVIDIA_PRISTINE_DIR:-/usr/src/nvidia-610.57.04.orig}
INSTALLED=${NVIDIA_INSTALLED_DIR:-/usr/src/nvidia-610.57.04}
F=src/nvidia-modeset/src/nvkms-evo.c
fails=0
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=$((fails + 1)); }
expect_rc() { # expect_rc WANT DESC cmd...
	local want=$1 desc=$2
	shift 2
	"$@" >"$T/out.txt" 2>&1
	local rc=$?
	if [ "$rc" = "$want" ]; then ok "$desc (rc $rc)"; else bad "$desc: rc $rc, wanted $want"; sed 's/^/     /' "$T/out.txt"; fi
}

T=$(mktemp -d "${TMPDIR:-/tmp}/nvpatch-test.XXXXXX")
trap 'chmod -R u+w "$T" 2>/dev/null; rm -rf "$T"' EXIT

# the pristine files: only the files the three patches touch are needed ($PRISTINE_TREE holds just those)
PATCHES=("$HERE/nvidia-2h1or-viewport.patch" "$HERE/nvidia-2h1or-flip-fix.patch" "$HERE/nvidia-evo-makeroom-bounded-wait.patch")
TOUCHED=$(cat "${PATCHES[@]}" | sed -n 's|^+++ b/||p' | sort -u)
PRISTINE_TREE=$T/pristine
mkdir -p "$PRISTINE_TREE"
if [ -f "$PRISTINE/$F" ]; then
	for f in $TOUCHED; do mkdir -p "$PRISTINE_TREE/$(dirname $f)"; cp -p "$PRISTINE/$f" "$PRISTINE_TREE/$f"; done
elif [ -f "$INSTALLED/$F" ]; then
	for f in $TOUCHED; do mkdir -p "$PRISTINE_TREE/$(dirname $f)"; cp -p "$INSTALLED/$f" "$PRISTINE_TREE/$f"; done
	ok_derive=1
	for ((i = 2; i >= 0; i--)); do patch -R -p1 -s -d "$PRISTINE_TREE" -i "${PATCHES[$i]}" >/dev/null 2>&1 || ok_derive=0; done
	if [ $ok_derive = 1 ]; then
		echo "note: no pristine copy at $PRISTINE, derived it from $INSTALLED with patch -R"
	else
		echo "SKIP: $INSTALLED is not fully patched and there is no pristine copy"
		exit 77
	fi
else
	echo "SKIP: neither $PRISTINE nor $INSTALLED found"
	exit 77
fi
PRISTINE_FILE=$PRISTINE_TREE/$F

# recording shims for the commands --rebuild runs
mkdir -p "$T/shims"
for n in dkms limine mkinitcpio; do
	printf '#!/bin/bash\necho "%s $*" >> "$SHIM_LOG"\n' "$n" >"$T/shims/$n"
	chmod +x "$T/shims/$n"
done
export SHIM_LOG=$T/shim.log
export NVIDIA_PATCH_DKMS=$T/shims/dkms NVIDIA_PATCH_LIMINE=$T/shims/limine NVIDIA_PATCH_MKINITCPIO=$T/shims/mkinitcpio

mk() { # mk NAME: scratch tree nvidia-610.57.04 under $T/NAME with the pristine file
	mkdir -p "$T/$1"
	cp -a "$PRISTINE_TREE" "$T/$1/nvidia-610.57.04"
}

# 1. pristine tree: --check says it needs the patch (2), a real run patches it (0)
mk a
R="$T/a"
expect_rc 2 "check on a pristine tree" "$SCRIPT" --check --src-root "$R"
grep -q "NOT patched" "$T/out.txt" && ok "check output says NOT patched" || bad "check output"
cmp -s "$R/nvidia-610.57.04/$F" "$PRISTINE_FILE" && ok "check changed nothing" || bad "check modified the file"
expect_rc 0 "patch a pristine tree" "$SCRIPT" --src-root "$R"
grep -q "originals kept" "$T/out.txt" && ok "run output says patched" || bad "run output"
cmp -s "$R/nvidia-610.57.04/$F.pimax-orig" "$PRISTINE_FILE" && ok "backup equals the pristine file" || bad "backup differs"
[ "$(diff -rq -x "*.pimax-orig" "$R/nvidia-610.57.04" "$PRISTINE_TREE" | grep -c "^Files")" = "$(echo "$TOUCHED" | wc -w)" ] && ok "every file the patches touch changed" || bad "unexpected number of patched files"
if [ -d "$INSTALLED" ]; then
	diffs=0
	for f in $TOUCHED; do cmp -s "$R/nvidia-610.57.04/$f" "$INSTALLED/$f" || diffs=$((diffs + 1)); done
	[ $diffs = 0 ] && ok "all touched files are byte-identical to the installed, hand-patched source" || bad "$diffs file(s) differ from $INSTALLED"
fi
[ ! -s "$SHIM_LOG" ] && ok "no rebuild command ran without --rebuild" || bad "rebuild commands ran: $(cat "$SHIM_LOG")"

# 2. idempotent
sum=$(sha256sum "$R/nvidia-610.57.04/$F")
expect_rc 0 "second run" "$SCRIPT" --src-root "$R"
grep -q "already patched" "$T/out.txt" && ok "second run says already patched" || bad "second run output"
[ "$sum" = "$(sha256sum "$R/nvidia-610.57.04/$F")" ] && ok "second run left the file alone" || bad "second run changed the file"
expect_rc 0 "check after patching" "$SCRIPT" --check --src-root "$R"

# 3. conflict: the context line the patch needs is gone
mk b
sed -i 's|EvoSetViewportPointIn(pDispEvo, head, 0 /\* x \*/, 0 /\* y \*/, updateState);|EvoSetViewportPointIn(pDispEvo, head, 0, 0, updateState);|' "$T/b/nvidia-610.57.04/$F"
expect_rc 1 "conflict is an error" "$SCRIPT" --src-root "$T/b"
grep -q "could NOT be applied" "$T/out.txt" && ok "conflict prints the banner" || bad "conflict banner"
expect_rc 1 "conflict in hook mode is an error too" "$SCRIPT" --hook --src-root "$T/b"

# 4. upstream fixed it differently (one line, or the call wrapped over two lines): not touched, not an error
mk c
sed -i 's|EvoSetViewportPointIn(pDispEvo, head, 0 /\* x \*/, 0 /\* y \*/, updateState);|EvoSetViewportPointIn(pDispEvo, head, pViewPort->in.width * pHeadState->mergeHeadSection, 0, updateState);|' "$T/c/nvidia-610.57.04/$F"
expect_rc 0 "fixed upstream is left alone" "$SCRIPT" --src-root "$T/c"
grep -q "not patching" "$T/out.txt" && ok "says not patching" || bad "fixed-upstream output"
mk c2
sed -i 's|EvoSetViewportPointIn(pDispEvo, head, 0 /\* x \*/, 0 /\* y \*/, updateState);|EvoSetViewportPointIn(pDispEvo, head,\n                          pViewPort->in.width * pHeadState->mergeHeadSection, 0, updateState);|' "$T/c2/nvidia-610.57.04/$F"
expect_rc 0 "an upstream fix wrapped over two lines is recognised too" "$SCRIPT" --src-root "$T/c2"
grep -q "not patching" "$T/out.txt" && ok "says not patching (wrapped call)" || bad "wrapped fixed-upstream output"

# 5. only versioned trees are considered; .orig copies and other versions are skipped; --version filters
mk d
cp -a "$T/d/nvidia-610.57.04" "$T/d/nvidia-610.57.04.orig"
cp -a "$T/d/nvidia-610.57.04" "$T/d/nvidia-999.0"
expect_rc 0 "several trees" "$SCRIPT" --src-root "$T/d" --version 610.57.04
cmp -s "$T/d/nvidia-610.57.04.orig/$F" "$PRISTINE_FILE" && ok ".orig copy untouched" || bad ".orig copy was modified"
cmp -s "$T/d/nvidia-999.0/$F" "$PRISTINE_FILE" && ok "other version untouched with --version" || bad "other version modified"
expect_rc 0 "all versions" "$SCRIPT" --src-root "$T/d"
cmp -s "$T/d/nvidia-999.0/$F" "$PRISTINE_FILE" && bad "other version not patched without --version" || ok "other version patched without --version"
cmp -s "$T/d/nvidia-610.57.04.orig/$F" "$PRISTINE_FILE" && ok ".orig copy still untouched" || bad ".orig copy modified by the all-versions run"

# 5b. a relative --patch path works although patch(1) changes into the tree first
mk g
( cd "$HERE" && "$SCRIPT" --check --src-root "$T/g" --patch nvidia-2h1or-viewport.patch >"$T/out.txt" 2>&1 )
rc=$?
[ "$rc" = 2 ] && grep -q "NOT patched" "$T/out.txt" && ok "relative --patch path (rc $rc)" || { bad "relative --patch path: rc $rc"; sed 's/^/     /' "$T/out.txt"; }

# 5c. --help shows the usage and nothing of the code
"$SCRIPT" --help >"$T/out.txt" 2>&1
grep -q "^Usage:" "$T/out.txt" && ! grep -q "set -euo" "$T/out.txt" && ok "--help prints only the usage" || bad "--help output"

# 6. nothing to do, missing patch, unwritable tree
mkdir -p "$T/e"
expect_rc 0 "no nvidia tree at all" "$SCRIPT" --src-root "$T/e"
expect_rc 1 "no nvidia tree in hook mode is loud" "$SCRIPT" --hook --src-root "$T/e"
grep -q "could NOT be applied" "$T/out.txt" && ok "hook mode prints the banner for a missing tree" || bad "hook-mode banner for a missing tree"
expect_rc 1 "missing patch file" "$SCRIPT" --src-root "$R" --patch "$T/none.patch"
if [ "$(id -u)" != 0 ]; then
	mk f
	chmod a-w "$T/f/nvidia-610.57.04/$F"
	expect_rc 1 "unwritable file explains itself" "$SCRIPT" --src-root "$T/f"
	grep -q "run as root" "$T/out.txt" && ok "mentions root" || bad "no root hint"
	mk f2
	chmod a-w "$T/f2/nvidia-610.57.04/$(dirname $F)" # file writable, directory not: patch(1) could not rename its temp file
	expect_rc 1 "unwritable directory explains itself" "$SCRIPT" --src-root "$T/f2"
	grep -q "run as root" "$T/out.txt" && ok "directory case mentions root" || bad "no root hint for the directory case"
	cmp -s "$T/f2/nvidia-610.57.04/$F" "$PRISTINE_FILE" && ok "nothing was changed" || bad "file changed in the unwritable directory"
fi

# 7. hook mode: a dkms tree whose layout moved is reported, a leftover dir without dkms.conf is not
mkdir -p "$T/h/nvidia-610.57.04"
touch "$T/h/nvidia-610.57.04/dkms.conf"
expect_rc 1 "hook mode: dkms tree without the target file is loud" "$SCRIPT" --hook --src-root "$T/h"
grep -q "not found in a dkms source tree" "$T/out.txt" && ok "says why" || bad "reason missing"
mkdir -p "$T/h2/nvidia-610.57.04"
expect_rc 0 "hook mode: a leftover directory without dkms.conf stays quiet" "$SCRIPT" --hook --src-root "$T/h2"
expect_rc 0 "normal mode: a tree without the target file is skipped" "$SCRIPT" --src-root "$T/h"

# 8. all-or-nothing: the viewport patch fits but the second one does not -> banner, rc 1, nothing changed
if true; then
	mk j
	sed -i 's|"Idling display engine timed out: 0x%08x:%d:%d:%d"|"Idling display engine timed out"|' "$T/j/nvidia-610.57.04/src/nvidia-modeset/src/nvkms-rm.c"
	sum=$(cd "$T/j" && find . -type f -print0 | sort -z | xargs -0 sha256sum | sha256sum)
	expect_rc 1 "second patch conflicts" "$SCRIPT" --hook --src-root "$T/j"
	grep -q "could NOT be applied" "$T/out.txt" && ok "banner for the second patch" || bad "no banner"
	[ "$sum" = "$(cd "$T/j" && find . -type f -print0 | sort -z | xargs -0 sha256sum | sha256sum)" ] && ok "tree untouched after the conflict" || bad "tree changed by a failed run"

	# 8b. only the viewport patch in (the old install): the other two are added
	mk k
	patch -p1 -s -d "$T/k/nvidia-610.57.04" -i "$PATCH"
	expect_rc 2 "check: viewport only" "$SCRIPT" --check --src-root "$T/k"
	grep -q "needs: nvidia-2h1or-flip-fix.patch nvidia-evo-makeroom" "$T/out.txt" && ok "names the two missing patches" || bad "missing-patch list"
	expect_rc 0 "add the other two" "$SCRIPT" --src-root "$T/k"
	expect_rc 0 "check after" "$SCRIPT" --check --src-root "$T/k"
	grep -q merged_head_sync "$T/k/nvidia-610.57.04/kernel-open/nvidia-modeset/nvidia-modeset-linux.c" && ok "merged_head_sync is in the source" || bad "no merged_head_sync"
fi

# 8c. --verify with a modinfo shim
printf '#!/bin/bash\necho "parm: merged_head_sync:x (uint)"\n' >"$T/shims/modinfo-ok"
printf '#!/bin/bash\necho "parm: other:x (uint)"\n' >"$T/shims/modinfo-bad"
chmod +x "$T/shims/modinfo-ok" "$T/shims/modinfo-bad"
NVIDIA_PATCH_MODINFO=$T/shims/modinfo-ok expect_rc 0 "verify: parameter present" "$SCRIPT" --verify
NVIDIA_PATCH_MODINFO=$T/shims/modinfo-bad expect_rc 1 "verify: parameter missing is an error" "$SCRIPT" --verify
grep -q "NO merged_head_sync" "$T/out.txt" && ok "verify banner" || bad "verify banner"

# 9. concurrent runs (pacman hook + manual run) do not clobber the backup
okc=0
for n in 1 2 3 4 5 6; do
	mk "race$n"
	( "$SCRIPT" --src-root "$T/race$n" >"$T/race$n.a" 2>&1 ) &
	( "$SCRIPT" --src-root "$T/race$n" >"$T/race$n.b" 2>&1 ) &
	wait
	if cmp -s "$T/race$n/nvidia-610.57.04/$F.pimax-orig" "$PRISTINE_FILE" && "$SCRIPT" --check --src-root "$T/race$n" >/dev/null 2>&1; then okc=$((okc + 1)); fi
done
[ "$okc" = 6 ] && ok "6 concurrent double runs: backup always the pristine file, tree patched" || bad "concurrent runs clobbered something ($okc of 6 clean)"

# 10. --rebuild: limine-mkinitcpio when present, mkinitcpio -P otherwise; dkms for the patched version only
mk r1
: >"$SHIM_LOG"
expect_rc 0 "rebuild with limine" "$SCRIPT" --rebuild --src-root "$T/r1"
grep -q "^dkms build nvidia/610.57.04 -k .* --force" "$SHIM_LOG" && grep -q "^dkms install nvidia/610.57.04" "$SHIM_LOG" && ok "dkms build + install ran" || bad "dkms calls: $(cat "$SHIM_LOG")"
grep -q "^limine" "$SHIM_LOG" && ! grep -q "^mkinitcpio" "$SHIM_LOG" && ok "limine-mkinitcpio used, not mkinitcpio -P" || bad "boot image command: $(cat "$SHIM_LOG")"
mk r2
: >"$SHIM_LOG"
NVIDIA_PATCH_LIMINE=/nonexistent/limine-mkinitcpio "$SCRIPT" --rebuild --src-root "$T/r2" >"$T/out.txt" 2>&1
grep -q "^mkinitcpio -P" "$SHIM_LOG" && ! grep -q "^limine" "$SHIM_LOG" && ok "mkinitcpio -P used when limine-mkinitcpio is absent" || bad "fallback: $(cat "$SHIM_LOG")"
: >"$SHIM_LOG"
"$SCRIPT" --rebuild --src-root "$T/r2" >"$T/out.txt" 2>&1
[ ! -s "$SHIM_LOG" ] && ok "nothing rebuilt when the tree was already patched" || bad "rebuilt an already patched tree: $(cat "$SHIM_LOG")"

# 11. --fallback: a failed patched build puts the stock source back and builds the stock module
mk r3
printf '#!/bin/bash\necho "dkms $*" >> "$SHIM_LOG"\n[ -e "$FAIL_FLAG" ] && [ "$1" = build ] && { rm -f "$FAIL_FLAG"; exit 1; }\nexit 0\n' >"$T/shims/dkms-fail"
chmod +x "$T/shims/dkms-fail"
: >"$SHIM_LOG"
export FAIL_FLAG=$T/fail-once
: >"$FAIL_FLAG"
NVIDIA_PATCH_NO_SYSLOG=1 NVIDIA_PATCH_DKMS=$T/shims/dkms-fail "$SCRIPT" --rebuild --fallback --src-root "$T/r3" >"$T/out.txt" 2>&1 && bad "fallback run should exit 1" || ok "fallback run exits 1"
cmp -s "$T/r3/nvidia-610.57.04/$F" "$PRISTINE_FILE" && ok "fallback: stock file back in place" || bad "fallback: file not restored"
[ -z "$(find "$T/r3" -name '*.pimax-orig')" ] && ok "fallback: no backups left" || bad "fallback: backups left"
[ "$(grep -c '^dkms build' "$SHIM_LOG")" = 2 ] && grep -q '^dkms install' "$SHIM_LOG" && ok "fallback: stock module built and installed" || bad "fallback dkms calls: $(cat "$SHIM_LOG")"
grep -q "STOCK module is installed" "$T/out.txt" && ok "fallback banner" || bad "fallback banner"
# --verify --fallback: dkms has no installed module and the tree is patched -> stock
mk r4
"$SCRIPT" --src-root "$T/r4" >/dev/null 2>&1
printf '#!/bin/bash\necho "dkms $*" >> "$SHIM_LOG"\n[ "$1" = status ] && exit 0\nexit 0\n' >"$T/shims/dkms-none"
chmod +x "$T/shims/dkms-none"
: >"$SHIM_LOG"
NVIDIA_SRC_ROOT=$T/r4 NVIDIA_PATCH_NO_SYSLOG=1 NVIDIA_PATCH_DKMS=$T/shims/dkms-none "$SCRIPT" --verify --fallback >"$T/out.txt" 2>&1 && bad "verify fallback should exit 1" || ok "verify fallback exits 1"
cmp -s "$T/r4/nvidia-610.57.04/$F" "$PRISTINE_FILE" && ok "verify fallback: stock file back" || bad "verify fallback: not restored"

echo
[ "$fails" = 0 ] && echo "all checks passed" || echo "$fails check(s) FAILED"
exit $((fails > 0))
