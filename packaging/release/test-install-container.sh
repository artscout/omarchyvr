#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# test-install-container.sh: the part of packaging/release/test-install.sh that runs INSIDE the disposable root container
# (image omarchyvr-release-builder, started with --cap-add SYS_NICE). Not meant to be run on a real PC: it creates a user,
# installs into that user's home and sets file capabilities.
#
# What it proves (release 0.1.1 failed it): after the app's root job did `setcap cap_sys_nice` on monado-service, the
# installed Monado could not load libonnxruntime ($ORIGIN RUNPATHs are ignored for a program with a file capability).
#
#   1. a fresh user extracts the tarball and runs ./install.sh --no-root --no-app --no-downloads (no user systemd bus here)
#   2. no installed ELF keeps $ORIGIN in its RUNPATH
#   3. as root: setcap cap_sys_nice=ep on the installed monado-service (what the app's root job does)
#   4. as the user, in secure mode (AT_SECURE): (a) the loader finds every library of monado-service and
#      `monado-service --help` does not die with "error while loading shared libraries"; (b) a setcap'd dlopen() test
#      program loads libbasalt.so, libopenxr_monado.so, libsherpa-onnx-c-api.so and the SteamVR driver_monado.so
#   5. the same checks on the unpatched ($ORIGIN) release tree itself must FAIL on (a): that proves the test catches the bug
#   6. ./install.sh again: the capability survives (an unchanged monado-service keeps its inode)
set -u
TB=${1:-/in/release.tar.zst}
fails=0
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=$((fails + 1)); }
info() { echo "info $*"; }
section() { echo; echo "== $*"; }

section "container"
echo "$(. /etc/os-release && echo "$PRETTY_NAME"), kernel $(uname -r), $(ldd --version | head -1), user $(id -un)"
[[ $EUID == 0 ]] || { echo "must run as root in the container" >&2; exit 2; }
useradd --create-home --shell /bin/bash tester || exit 2
as_tester() { runuser -u tester -- env -i HOME=/home/tester PATH=/usr/local/bin:/usr/bin LANG=C.UTF-8 "$@"; }
cp "$TB" /tmp/release.tar.zst && chmod 644 /tmp/release.tar.zst
as_tester sh -c 'mkdir -p "$HOME/rel" && zstd -dc /tmp/release.tar.zst | tar -xf - -C "$HOME/rel"' || { echo "cannot extract the tarball" >&2; exit 2; }
R=$(find /home/tester/rel -mindepth 1 -maxdepth 1 -type d | head -1)
echo "release tree: $R ($(grep -m1 '^version:' "$R/MANIFEST.txt"), commit $(grep -m1 '^commit:' "$R/MANIFEST.txt" | cut -c1-17))"
PREFIX=/home/tester/.local
REL_SVC=$R/lib/omarchyvr/monado/src/xrt/targets/service/monado-service
SVC=$PREFIX/lib/omarchyvr/monado/src/xrt/targets/service/monado-service
LIBS_REL=(lib/omarchyvr/basalt/lib/libbasalt.so lib/omarchyvr/monado/src/xrt/targets/openxr/libopenxr_monado.so
	lib/omarchyvr/sherpa-onnx/lib/libsherpa-onnx-c-api.so lib/omarchyvr/monado/steamvr-monado/bin/linux64/driver_monado.so)

# the dlopen() test program, with the file capability set on a copy (secure mode) and without (control)
cat >/tmp/dl.c <<'C'
#include <dlfcn.h>
#include <stdio.h>
#include <sys/auxv.h>
int main(int c, char **v) {
	void *h = dlopen(v[1], RTLD_NOW | RTLD_GLOBAL);
	if (!h) { printf("dlopen FAILED (AT_SECURE=%lu) %s: %s\n", getauxval(AT_SECURE), v[1], dlerror()); return 1; }
	printf("dlopen ok (AT_SECURE=%lu) %s\n", getauxval(AT_SECURE), v[1]);
	return 0;
}
C
gcc -o /tmp/dl-plain /tmp/dl.c -ldl && cp /tmp/dl-plain /tmp/dl-secure && chmod 755 /tmp/dl-plain /tmp/dl-secure || { echo "cannot compile the dlopen test" >&2; exit 2; }
setcap cap_sys_nice=ep /tmp/dl-secure || { echo "setcap does not work in this container (--cap-add SYS_NICE / SETFCAP?)" >&2; exit 2; }

# check_tree LABEL SERVICE TREE: sets A_FAIL (monado-service) and B_FAIL (dlopen) to the number of failures; the service
# must already have its capability
check_tree() {
	local label=$1 svc=$2 tree=$3 out l
	A_FAIL=0 B_FAIL=0
	out=$(as_tester sh -c 'LD_TRACE_LOADED_OBJECTS=1 exec "$0"' "$svc" 2>&1)
	if grep -qE 'error while loading shared libraries|not found' <<<"$out"; then
		echo "FAIL [$label] (a) loader, secure mode: $(grep -E 'error while loading|not found' <<<"$out" | head -2 | tr '\n' ' ')"; A_FAIL=$((A_FAIL + 1))
	else echo "ok   [$label] (a) loader finds every library of monado-service in secure mode"; fi
	out=$(as_tester timeout 5 "$svc" --help 2>&1)
	if grep -q 'error while loading shared libraries' <<<"$out"; then
		echo "FAIL [$label] (a) monado-service --help: $(grep 'error while loading' <<<"$out" | head -1)"; A_FAIL=$((A_FAIL + 1))
	else echo "ok   [$label] (a) monado-service --help starts (no shared library error)"; fi
	for l in "${LIBS_REL[@]}"; do
		if ! as_tester /tmp/dl-plain "$tree/$l" >/dev/null 2>&1; then info "[$label] control: $l does not even load without the capability (not a secure-mode problem)"; fi
		if out=$(as_tester /tmp/dl-secure "$tree/$l" 2>&1); then echo "ok   [$label] (b) $out" | sed "s|$tree/||"; else echo "FAIL [$label] (b) $out" | sed "s|$tree/||"; B_FAIL=$((B_FAIL + 1)); fi
	done
}

section "1. install.sh as the fresh user (no user systemd bus in this container)"
as_tester bash -c "cd '$R' && ./install.sh --no-root --no-app --no-downloads" >/tmp/install.log 2>&1
rc=$?
sed 's/^/    | /' /tmp/install.log | tail -14
[[ $rc == 0 ]] && ok "install.sh ended with rc 0" || bad "install.sh ended with rc $rc (the half-way abort of release 0.1.1)"
if [[ ! -x $SVC ]]; then echo "FAIL monado-service was not installed: nothing more to test"; exit 1; fi

section "2. RUNPATHs of the installed files"
n=$(find "$PREFIX/lib/omarchyvr" "$PREFIX/bin" -type f -print0 | xargs -0 -r readelf -d 2>/dev/null | grep -cE '(RUNPATH|RPATH).*ORIGIN')
[[ $n == 0 ]] && ok "no installed ELF has \$ORIGIN in its RUNPATH" || bad "$n installed RUNPATH/RPATH entries still contain \$ORIGIN"
echo "    monado-service RUNPATH: $(readelf -d "$SVC" | sed -n 's/.*\(RUNPATH\|RPATH\).*\[\(.*\)\]/\2/p')"
[[ -f $PREFIX/share/omarchyvr/runpath-rewrite.tsv ]] && ok "the rewrite list exists ($(grep -vc '^#' "$PREFIX/share/omarchyvr/runpath-rewrite.tsv") files)" || bad "no share/omarchyvr/runpath-rewrite.tsv"

section "3. setcap cap_sys_nice=ep on the installed monado-service (the app's root job)"
setcap cap_sys_nice=ep "$SVC" && echo "    $(getcap "$SVC")" || { echo "setcap failed" >&2; exit 2; }
INODE=$(stat -c %i "$SVC")

section "4. secure mode, installed tree"
check_tree installed "$SVC" "$PREFIX"
[[ $A_FAIL == 0 && $B_FAIL == 0 ]] && ok "installed tree: every check passed" || bad "installed tree: $A_FAIL (a) and $B_FAIL (b) failures"

section "5. the same checks on the unpatched release tree (they must FAIL: this proves the test catches the 0.1.1 bug)"
as_tester cp -a "$R" /home/tester/unpatched
setcap cap_sys_nice=ep /home/tester/unpatched/lib/omarchyvr/monado/src/xrt/targets/service/monado-service
echo "    release RUNPATH: $(readelf -d "$REL_SVC" | sed -n 's/.*\(RUNPATH\|RPATH\).*\[\(.*\)\]/\2/p')"
check_tree unpatched /home/tester/unpatched/lib/omarchyvr/monado/src/xrt/targets/service/monado-service /home/tester/unpatched
if readelf -d "$REL_SVC" | grep -qE '(RUNPATH|RPATH).*ORIGIN'; then
	[[ $A_FAIL -gt 0 ]] && ok "unpatched \$ORIGIN tree: (a) FAILS under secure mode, as expected (the test catches the bug)" || bad "unpatched \$ORIGIN tree passed (a): this test cannot catch the 0.1.1 bug"
	info "(b) on the unpatched tree: $B_FAIL failure(s). glibc ignores only the setcap'd EXECUTABLE's own \$ORIGIN RUNPATH; a library's own \$ORIGIN RUNPATH still works when it is dlopen()ed"
else
	info "the release tree has no \$ORIGIN RUNPATH on monado-service (an absolute-RUNPATH build): the unpatched-tree check is not meaningful"
fi

section "6. a second ./install.sh keeps the capability (unchanged monado-service keeps its inode)"
as_tester bash -c "cd '$R' && ./install.sh --no-root --no-app --no-downloads" >/tmp/install2.log 2>&1
rc2=$?
[[ $rc2 == 0 ]] && ok "second install.sh ended with rc 0" || bad "second install.sh rc $rc2"
[[ $(stat -c %i "$SVC") == "$INODE" ]] && getcap "$SVC" | grep -q cap_sys_nice && ok "monado-service kept its inode and cap_sys_nice" || bad "monado-service lost its capability on reinstall"
out=$(as_tester sh -c 'LD_TRACE_LOADED_OBJECTS=1 exec "$0"' "$SVC" 2>&1)
grep -qE 'error while loading|not found' <<<"$out" && bad "after the reinstall the loader fails again" || ok "after the reinstall the loader still finds every library"

echo
if [[ $fails == 0 ]]; then echo "test-install: PASSED"; else echo "test-install: $fails check(s) FAILED"; fi
[[ $fails == 0 ]]
