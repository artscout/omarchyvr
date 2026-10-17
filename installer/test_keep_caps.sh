#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Test of cap_keep_begin / cap_keep_end (lib.sh): the realtime permission of the installed monado-service survives a
# rebuild that relinks it to the same bytes (real install 2026-10-17 13:22: every ./install.sh asked for the password
# again). No root: a file capability cannot be set here, so has_nice_cap is replaced by a list of "capable" inodes
# (a file capability belongs to the inode, which is exactly what the helpers rely on).
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
. "$HERE/lib.sh"
fails=0
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=$((fails + 1)); }
T=$(mktemp -d "${TMPDIR:-/tmp}/keep-caps-test.XXXXXX")
trap 'rm -rf "$T"' EXIT
CAPS=$T/capable-inodes
has_nice_cap() { [[ -e $1 ]] && grep -qx "$(stat -c %i -- "$1")" "$CAPS" 2>/dev/null; }
setcap_fake() { stat -c %i -- "$1" >>"$CAPS"; }
S=$T/monado/src/xrt/targets/service/monado-service
relink() { # relink CONTENT: what a linker does: a new file (new inode) under the same name
	local tmp=$S.ld-tmp
	printf '%s' "$1" >"$tmp"
	chmod 755 "$tmp"
	mv -f "$tmp" "$S"
}
fresh() {
	rm -rf "$T/monado" "$CAPS"
	mkdir -p "$(dirname "$S")"
	printf 'binary-v1' >"$S"
	chmod 755 "$S"
	setcap_fake "$S"
}

# 1. ninja did nothing: same file, permission untouched
fresh
cap_keep_begin "$S"
r=$(cap_keep_end "$S")
if [[ $r == unchanged ]] && has_nice_cap "$S" && [[ ! -e $S.cap-keep ]]; then ok "not relinked: unchanged, permission kept, no leftover"; else bad "not relinked: $r"; fi

# 2. relinked to the same bytes (inputs touched, nothing really changed): the old inode goes back
fresh
ino=$(stat -c %i "$S")
cap_keep_begin "$S"
sleep 1.1
relink 'binary-v1'
if has_nice_cap "$S"; then bad "the fake relink kept the permission (test broken)"; fi
r=$(cap_keep_end "$S")
if [[ $r == kept && $(stat -c %i "$S") == "$ino" ]] && has_nice_cap "$S" && [[ ! -e $S.cap-keep && $(cat "$S") == binary-v1 ]]; then
	ok "relinked to the same bytes: kept (old inode back, permission kept)"
else bad "same bytes: $r"; fi
# its mtime is new, so ninja does not see it as older than its inputs
if [[ $(($(date +%s) - $(stat -c %Y "$S"))) -le 2 ]]; then ok "the restored file has a fresh mtime (no relink next time)"; else bad "mtime not refreshed"; fi

# 3. a real update: the new binary stays, the permission is gone and the caller hears "changed"
fresh
cap_keep_begin "$S"
relink 'binary-v2'
r=$(cap_keep_end "$S")
if [[ $r == changed && $(cat "$S") == binary-v2 && ! -e $S.cap-keep ]] && ! has_nice_cap "$S"; then ok "changed binary: new file stays, says changed"; else bad "changed: $r"; fi

# 4. no permission before (first install): none, nothing kept
fresh
: >"$CAPS"
cap_keep_begin "$S"
[[ -e $S.cap-keep ]] && bad "kept a file that had no permission"
relink 'binary-v1'
r=$(cap_keep_end "$S")
if [[ $r == none ]] && ! has_nice_cap "$S"; then ok "no permission before: none"; else bad "none: $r"; fi

# 5. no binary yet (very first build): none
rm -rf "$T/monado" "$CAPS"
mkdir -p "$(dirname "$S")"
cap_keep_begin "$S"
relink 'binary-v1'
r=$(cap_keep_end "$S")
if [[ $r == none ]]; then ok "first build: none"; else bad "first build: $r"; fi

# 6. a leftover keep file from an interrupted install is replaced, never trusted
fresh
printf 'stale' >"$S.cap-keep"
cap_keep_begin "$S"
if [[ $(stat -c %i "$S.cap-keep") == "$(stat -c %i "$S")" ]]; then ok "a stale keep file is replaced by a link to the current binary"; else bad "stale keep file kept"; fi
cap_keep_end "$S" >/dev/null

# 7. the build removed the binary and failed to write a new one: the old file goes back
fresh
cap_keep_begin "$S"
rm -f "$S"
r=$(cap_keep_end "$S")
if [[ -e $S ]] && has_nice_cap "$S"; then ok "binary removed by a failed link: the old one goes back ($r)"; else bad "removed binary: $r"; fi

# install.sh wires them around the Monado compile and writes the app's post-install check
if grep -q 'cap_keep_begin "$SERVICE"' "$HERE/install.sh" && grep -q 'CAP_STATE=$(cap_keep_end "$SERVICE")' "$HERE/install.sh" &&
	grep -q 'post-install-check' "$HERE/install.sh"; then
	ok "install.sh keeps the permission around the Monado build and leaves the post-install check for the app"
else bad "install.sh is not wired"; fi
# and refuses to build Monado in place while a session runs
if awk '/pgrep -x monado-service/ { p = NR } /build "Monado \(forks\/monado, configure\)"/ { c = NR } END { exit !(p && c && p < c) }' "$HERE/install.sh"; then
	ok "install.sh checks for a running session right before building Monado in place"
else bad "no running-session check before the Monado build"; fi

echo
if [ "$fails" = 0 ]; then echo "all checks passed"; else echo "$fails check(s) FAILED"; fi
[ "$fails" = 0 ]
