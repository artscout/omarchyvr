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
# never touch the real conf / status files (the test may run as root)
export NVIDIA_PATCH_CONF=$T/nvidia.conf NVIDIA_PATCH_STATUS=$T/status/nvidia-patch-status
unset NVIDIA_PATCH_TESTED

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
# the real modinfo of the machine running the test must not decide the --rebuild tests
printf '#!/bin/bash\necho "parm: merged_head_sync:x (uint)"\n' >"$T/shims/modinfo-default"
chmod +x "$T/shims/modinfo-default"
export NVIDIA_PATCH_MODINFO=$T/shims/modinfo-default

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
if [ -d "$INSTALLED" ] && grep -q merged_head_sync "$INSTALLED/kernel-open/nvidia-modeset/nvidia-modeset-linux.c" 2>/dev/null; then # only when the installed tree is patched
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
expect_rc 0 "all versions (999.0 is untested: refused without the opt-in)" "$SCRIPT" --src-root "$T/d"
cmp -s "$T/d/nvidia-999.0/$F" "$PRISTINE_FILE" && ok "untested other version left alone without the opt-in" || bad "untested version was patched without the opt-in"
expect_rc 0 "all versions with --allow-untested" "$SCRIPT" --allow-untested --src-root "$T/d"
cmp -s "$T/d/nvidia-999.0/$F" "$PRISTINE_FILE" && bad "other version not patched without --version" || ok "other version patched without --version (opted in)"
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


# 12. untested drivers: refused by default, allowed by flag or conf file, fuzz fallback, status file
st() { sed -n "s/^$1=//p" "$NVIDIA_PATCH_STATUS" | head -n1; }
mkv() { # mkv NAME VERSION: scratch tree nvidia-VERSION under $T/NAME
	mkdir -p "$T/$1"
	cp -a "$PRISTINE_TREE" "$T/$1/nvidia-$2"
}
FLIPF=kernel-open/nvidia-modeset/nvidia-modeset-linux.c
grep -qx 610.57.04 "$HERE/tested-versions.txt" && ok "tested-versions.txt lists 610.57.04" || bad "tested-versions.txt"

# 12a. refused by default: nothing patched, non-fatal (pacman hook), banner, status
rm -f "$NVIDIA_PATCH_CONF" "$NVIDIA_PATCH_STATUS"
mkv u1 999.1
expect_rc 0 "untested driver: refused, exit 0" "$SCRIPT" --src-root "$T/u1"
grep -q "NVIDIA driver 999.1 is not tested" "$T/out.txt" && grep -q "Allow the patch on untested NVIDIA drivers" "$T/out.txt" && ok "refusal banner names the app switch" || bad "refusal banner"
cmp -s "$T/u1/nvidia-999.1/$F" "$PRISTINE_FILE" && [ -z "$(find "$T/u1" -name '*.pimax-orig')" ] && ok "refused: tree untouched, no backups" || bad "refused tree was changed"
[ "$(st driver)" = 999.1 ] && [ "$(st result)" = untested-refused ] && [ "$(st fuzz)" = - ] && ok "status: driver 999.1, untested-refused, fuzz -" || bad "status after refusal: $(cat "$NVIDIA_PATCH_STATUS" 2>&1)"
expect_rc 0 "untested driver in hook mode: exit 0 (pacman is not blocked)" "$SCRIPT" --hook --src-root "$T/u1"
expect_rc 0 "untested driver, --check: nothing to do" "$SCRIPT" --check --src-root "$T/u1"
: >"$SHIM_LOG"
expect_rc 0 "untested driver with --rebuild --fallback builds nothing" "$SCRIPT" --hook --rebuild --fallback --src-root "$T/u1"
[ ! -s "$SHIM_LOG" ] && ok "refused: no dkms / boot image command ran (the stock module is dkms's own job)" || bad "refused run called: $(cat "$SHIM_LOG")"
NVIDIA_SRC_ROOT=$T/u1 expect_rc 0 "--verify after a refusal is calm (exit 0)" "$SCRIPT" --verify --hook --fallback
printf '#!/bin/bash\necho "parm: other:x (uint)"\n' >"$T/shims/modinfo-bad2"
chmod +x "$T/shims/modinfo-bad2"
NVIDIA_PATCH_MODINFO=$T/shims/modinfo-bad2 expect_rc 0 "--verify after a refusal does not complain about the stock module" "$SCRIPT" --verify
grep -q "untested" "$T/out.txt" && ok "verify says why" || bad "verify refusal text"

# 12b. allowed by the flag: loud warning, patched, status untested-allowed with fuzz 0
mkv u2 999.2
expect_rc 0 "untested driver with --allow-untested" "$SCRIPT" --allow-untested --src-root "$T/u2"
grep -q "WARNING: NVIDIA driver 999.2 is NOT tested" "$T/out.txt" && ok "loud WARNING printed" || bad "warning missing"
! cmp -s "$T/u2/nvidia-999.2/$F" "$PRISTINE_FILE" && grep -q merged_head_sync "$T/u2/nvidia-999.2/$FLIPF" && ok "allowed: patched, merged_head_sync present" || bad "allowed tree not patched"
[ "$(st driver)" = 999.2 ] && [ "$(st result)" = untested-allowed ] && [ "$(st fuzz)" = 0 ] && ok "status: untested-allowed, fuzz 0" || bad "status after allowed run: $(cat "$NVIDIA_PATCH_STATUS")"
expect_rc 0 "second allowed run" "$SCRIPT" --allow-untested --src-root "$T/u2"
[ "$(st result)" = untested-allowed ] && [ "$(st fuzz)" = 0 ] && ok "status keeps fuzz 0 when nothing was applied" || bad "status after second run"

# 12c. allowed by the conf file (what the app's switch writes); off by "allow_untested=0"
mkv u3 999.3
printf '# omarchyvr\nallow_untested=0\n' >"$NVIDIA_PATCH_CONF"
expect_rc 0 "conf allow_untested=0: refused" "$SCRIPT" --src-root "$T/u3"
[ "$(st result)" = untested-refused ] && cmp -s "$T/u3/nvidia-999.3/$F" "$PRISTINE_FILE" && ok "conf off: refused" || bad "conf off"
printf '# omarchyvr\nallow_untested=1\n' >"$NVIDIA_PATCH_CONF"
expect_rc 0 "conf allow_untested=1: patched" "$SCRIPT" --hook --src-root "$T/u3"
[ "$(st result)" = untested-allowed ] && grep -q merged_head_sync "$T/u3/nvidia-999.3/$FLIPF" && ok "conf on: patched" || bad "conf on"
printf 'allow_untested=1; touch %s/pwned\n' "$T" >"$NVIDIA_PATCH_CONF"
mkv u3b 999.31
expect_rc 0 "a malicious conf line is not executed and not honoured" "$SCRIPT" --src-root "$T/u3b"
[ ! -e "$T/pwned" ] && cmp -s "$T/u3b/nvidia-999.31/$F" "$PRISTINE_FILE" && ok "conf is parsed, never sourced" || bad "conf line was executed or honoured"
rm -f "$NVIDIA_PATCH_CONF"

# 12d. fuzz fallback: a context line at the edge of a hunk differs
fuzzed() { sed -i 's|^\(    pDevEvo->hal->SetOutputScaler(pDispEvo, head, updateState);\)$|\1 /* vendor tweak */|' "$1/$F"; grep -q 'vendor tweak' "$1/$F"; }
mkv u4 999.4
fuzzed "$T/u4/nvidia-999.4" && ok "test setup: edge context line changed" || bad "fuzz test setup failed"
expect_rc 0 "untested + opted in, fuzz 0 fails: fuzz 2 patches it" "$SCRIPT" --allow-untested --src-root "$T/u4"
grep -q "trying --fuzz=2" "$T/out.txt" && grep -q "apply only with --fuzz=2" "$T/out.txt" && ok "says fuzz 2 was needed" || bad "fuzz 2 not announced"
[ "$(st result)" = untested-allowed ] && [ "$(st fuzz)" = 2 ] && ok "status: fuzz 2" || bad "status after fuzz: $(cat "$NVIDIA_PATCH_STATUS")"
grep -q merged_head_sync "$T/u4/nvidia-999.4/$FLIPF" && grep -q "mergeHeadSection" "$T/u4/nvidia-999.4/$F" && ok "fuzz 2: all patches in" || bad "fuzz 2 result incomplete"
expect_rc 0 "second run on the fuzz-patched tree" "$SCRIPT" --allow-untested --src-root "$T/u4"
grep -q "already patched" "$T/out.txt" && [ "$(st fuzz)" = 2 ] && ok "already patched, fuzz 2 remembered" || bad "second run on fuzz tree: $(cat "$T/out.txt")"
mkv u4b 999.41
fuzzed "$T/u4b/nvidia-999.41"
expect_rc 2 "check with the opt-in says it needs fuzz 2" "$SCRIPT" --allow-untested --check --src-root "$T/u4b"
grep -q "only with --fuzz=2" "$T/out.txt" && ok "check names fuzz 2" || bad "check text"
mkv u4c 999.42
fuzzed "$T/u4c/nvidia-999.42"
expect_rc 0 "fuzzed untested tree without the opt-in: refused" "$SCRIPT" --src-root "$T/u4c"
cmp -s "$T/u4c/nvidia-999.42/$F" "$PRISTINE_FILE" || [ "$(grep -c 'vendor tweak' "$T/u4c/nvidia-999.42/$F")" = 1 ] && ok "not patched without the opt-in" || bad "patched without opt-in"
[ -z "$(find "$T/u4c" -name '*.pimax-orig')" ] || bad "backups made without the opt-in"
# a TESTED version never gets the fuzz retry
mkv u4d 610.57.04
fuzzed "$T/u4d/nvidia-610.57.04"
expect_rc 1 "tested version with a changed context line: conflict, no fuzz" "$SCRIPT" --src-root "$T/u4d"
! grep -q "fuzz=2" "$T/out.txt" && [ "$(st result)" = failed ] && ok "tested version: no fuzz retry, status failed" || bad "tested version took the fuzz path"
# conflict even with fuzz 2
mkv u4e 999.43
sed -i 's|EvoSetViewportPointIn(pDispEvo, head, 0 /\* x \*/, 0 /\* y \*/, updateState);|EvoSetViewportPointIn(pDispEvo, head, 0, 0, updateState);|' "$T/u4e/nvidia-999.43/$F"
expect_rc 1 "opted in, but the code changed too much: conflict" "$SCRIPT" --allow-untested --src-root "$T/u4e"
grep -q "could NOT be applied" "$T/out.txt" && grep -q "also tried --fuzz=2" "$T/out.txt" && [ "$(st result)" = failed ] && ok "conflict: banner, status failed" || bad "conflict handling: $(cat "$T/out.txt")"
[ -z "$(find "$T/u4e" -name '*.pimax-orig')" ] && ok "conflict: tree untouched" || bad "conflict left backups"

# 12e. the tested list can be overridden (a version added to the list needs no opt-in)
mkv u5 999.5
printf '# test\n610.57.04\n999.5  # added\n' >"$T/tested.txt"
NVIDIA_PATCH_TESTED=$T/tested.txt expect_rc 0 "version in the tested list: patched without the opt-in" "$SCRIPT" --src-root "$T/u5"
[ "$(st result)" = tested ] && [ "$(st fuzz)" = 0 ] && grep -q merged_head_sync "$T/u5/nvidia-999.5/$FLIPF" && ok "listed version: status tested" || bad "listed version"
! grep -q "WARNING" "$T/out.txt" && ok "no warning for a tested version" || bad "tested version warned"
mkv u5b 610.57.04
NVIDIA_PATCH_TESTED=$T/none.txt expect_rc 0 "a missing list falls back to 610.57.04" "$SCRIPT" --src-root "$T/u5b"
[ "$(st result)" = tested ] && ok "built-in default lists 610.57.04" || bad "built-in default"

# 12f. opted in, the patched build fails: stock module comes back, status failed
mkv u6 999.6
: >"$SHIM_LOG"
: >"$FAIL_FLAG"
NVIDIA_PATCH_DKMS=$T/shims/dkms-fail "$SCRIPT" --allow-untested --hook --rebuild --fallback --src-root "$T/u6" >"$T/out.txt" 2>&1 && bad "failed untested build should exit 1" || ok "failed untested build exits 1"
cmp -s "$T/u6/nvidia-999.6/$F" "$PRISTINE_FILE" && [ "$(grep -c '^dkms build' "$SHIM_LOG")" = 2 ] && ok "untested: stock source back and stock module built" || bad "untested fallback"
[ "$(st result)" = failed ] && [ "$(st driver)" = 999.6 ] && ok "status: failed" || bad "status after failed build: $(cat "$NVIDIA_PATCH_STATUS")"

# 12g. without write access to the status file nothing breaks
mkv u7 999.7
NVIDIA_PATCH_STATUS=/proc/none/status expect_rc 0 "unwritable status file is not an error" "$SCRIPT" --allow-untested --src-root "$T/u7"

# 13. per-version patches and "ported" versions (<dir>/<version>/<name> overrides the default set)
HP=$T/hp
mkdir -p "$HP/998.1" "$HP/998.3"
cp "$HERE"/nvidia-*.patch "$SCRIPT" "$HP/"
HS=$HP/nvidia-patch-reapply.sh
printf '# test\n998.1\n998.3  # ported\n' >"$T/ported.txt"
# 998.1: the line the default viewport patch removes differs in this driver (an extra blank); its port has the same change
sed 's|^\(-    EvoSetViewportPointIn(.*0 /\* y \*/,\) updateState);|\1  updateState);|' "$HERE/nvidia-2h1or-viewport.patch" >"$HP/998.1/nvidia-2h1or-viewport.patch"
! cmp -s "$HP/998.1/nvidia-2h1or-viewport.patch" "$HERE/nvidia-2h1or-viewport.patch" && ok "ported setup: the port differs from the default patch" || bad "ported setup failed"
portedtree() { # portedtree NAME VERSION
	mkv "$1" "$2"
	sed -i 's|^\(    EvoSetViewportPointIn(.*0 /\* y \*/,\) updateState);|\1  updateState);|' "$T/$1/nvidia-$2/$F"
}
export NVIDIA_PATCH_PORTED=$T/ported.txt
portedtree p1 998.1
expect_rc 0 "ported version without the opt-in: refused" "$HS" --src-root "$T/p1"
grep -q "Patches ported to 998.1 exist" "$T/out.txt" && ok "refusal banner mentions the port" || bad "refusal banner without port hint"
[ "$(st result)" = untested-refused ] && ok "ported version is still untested (needs the opt-in)" || bad "ported version status: $(cat "$NVIDIA_PATCH_STATUS")"
expect_rc 0 "ported version with the opt-in uses the port" "$HS" --allow-untested --src-root "$T/p1"
grep -q "using the patches ported to 998.1" "$T/out.txt" && ok "says the port is used" || bad "port not announced"
grep -q merged_head_sync "$T/p1/nvidia-998.1/$FLIPF" && grep -q mergeHeadSection "$T/p1/nvidia-998.1/$F" && ok "ported: all three patches in" || bad "ported tree incomplete"
[ "$(st result)" = untested-allowed ] && [ "$(st fuzz)" = 0 ] && [ "$(st port)" = ported ] && ok "status: untested-allowed, fuzz 0, port=ported" || bad "ported status: $(cat "$NVIDIA_PATCH_STATUS")"
expect_rc 0 "ported tree, second run" "$HS" --allow-untested --src-root "$T/p1"
grep -q "already patched" "$T/out.txt" && ok "ported: idempotent" || bad "ported second run"
# the same tree under a version without a port: the default patch does not fit, in any fuzz
portedtree p2 998.2
expect_rc 1 "same source, version without a port: default patch conflicts" "$HS" --allow-untested --src-root "$T/p2"
! grep -q "using the patches ported" "$T/out.txt" && ok "no port used for an unlisted version" || bad "unlisted version used a port"
# a ported version never takes the relaxed --fuzz=2 retry (the default patch would fit with fuzz 2 here)
cp "$HERE/nvidia-2h1or-viewport.patch" "$HP/998.3/"
mkv p3 998.3
fuzzed "$T/p3/nvidia-998.3"
expect_rc 1 "ported version with a changed context line: conflict, no fuzz" "$HS" --allow-untested --src-root "$T/p3"
! grep -q "trying --fuzz=2" "$T/out.txt" && [ "$(st result)" = failed ] && ok "ported: no fuzz retry, status failed" || bad "ported version took the fuzz path: $(cat "$T/out.txt")"
mkv p4 998.4
fuzzed "$T/p4/nvidia-998.4"
expect_rc 0 "unlisted version, same change: fuzz 2 still available" "$HS" --allow-untested --src-root "$T/p4"
[ "$(st fuzz)" = 2 ] && ok "unlisted version still uses fuzz 2" || bad "unlisted fuzz"
# the shipped list and directory agree
unset NVIDIA_PATCH_PORTED
for v in $(sed 's/#.*//' "$HERE/ported-versions.txt"); do
	for n in nvidia-2h1or-viewport.patch nvidia-2h1or-flip-fix.patch nvidia-evo-makeroom-bounded-wait.patch; do
		[ -f "$HERE/$v/$n" ] && ok "ported $v ships $n" || bad "ported $v lacks $n"
	done
	! grep -qx "$v" <(sed 's/#.*//; s/[[:space:]]//g' "$HERE/tested-versions.txt") && ok "ported $v is not in tested-versions.txt (no hardware test yet)" || bad "ported $v is listed as tested"
done

# 13b. the real 615.78.08 source (the stock dkms tree, e.g. /usr/src/nvidia-615.78.08 or an extracted package) when given
if [ -n "${NVIDIA_615_DIR:-}" ] && [ -f "$NVIDIA_615_DIR/$F" ]; then
	mkdir -p "$T/r615"
	cp -a "$NVIDIA_615_DIR" "$T/r615/nvidia-615.78.08"
	expect_rc 0 "615.78.08 without the opt-in: refused (not hardware-tested)" "$SCRIPT" --src-root "$T/r615"
	expect_rc 0 "615.78.08 with the opt-in: the port applies" "$SCRIPT" --allow-untested --src-root "$T/r615"
	[ "$(st result)" = untested-allowed ] && [ "$(st fuzz)" = 0 ] && [ "$(st port)" = ported ] && ok "615.78.08: fuzz 0, port=ported" || bad "615 status: $(cat "$NVIDIA_PATCH_STATUS")"
	grep -q merged_head_sync "$T/r615/nvidia-615.78.08/$FLIPF" && grep -q mergeHeadSection "$T/r615/nvidia-615.78.08/$F" && ok "615.78.08: merged_head_sync + viewport in" || bad "615 patches incomplete"
	for p in "$HERE/615.78.08"/*.patch; do
		patch -R -p1 --dry-run --fuzz=0 -s -d "$T/r615/nvidia-615.78.08" -i "$p" >/dev/null 2>&1 && ok "615.78.08: $(basename "$p") reverses cleanly with --fuzz=0" || bad "615 $(basename "$p") does not reverse"
	done
else
	echo "note: NVIDIA_615_DIR not set, the real 615.78.08 source test is skipped"
fi

echo
[ "$fails" = 0 ] && echo "all checks passed" || echo "$fails check(s) FAILED"
exit $((fails > 0))
