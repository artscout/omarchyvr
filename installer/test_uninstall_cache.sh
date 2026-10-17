#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Test of the uninstaller's handling of the installer's build cache, as a DRY RUN in a scratch HOME (nothing changes,
# no password, the real home is never touched): ~/.cache/omarchyvr/build is removed as a build cache BEFORE the settings
# backup is made, so it never goes into that tar (real uninstall 2026-10-17 12:01: a 2.5 GB "settings" backup).
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
fails=0
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=$((fails + 1)); }
T=$(mktemp -d "${TMPDIR:-/tmp}/uninstall-cache-test.XXXXXX")
trap 'rm -rf "$T"' EXIT
H=$T/home
mkdir -p "$H/.cache/omarchyvr/build/cargo/release" "$H/.cache/omarchyvr/thumbs" "$H/.config/omarchyvr" "$H/.local/share"
echo x >"$H/.cache/omarchyvr/build/cargo/release/libbig.a"
echo y >"$H/.config/omarchyvr/gui-state.toml"
out=$(env -u XDG_CONFIG_HOME -u XDG_DATA_HOME -u XDG_STATE_HOME -u XDG_CACHE_HOME HOME="$H" TMPDIR="$T" \
	"$HERE/uninstall.sh" --dry-run --no-root --yes 2>&1)
rc=$?
[[ $rc == 0 ]] && ok "dry run ends cleanly" || bad "dry run rc=$rc: $(tail -5 <<<"$out")"
bline=$(grep -n "BUILD .*remove .*$H/.cache/omarchyvr/build" <<<"$out" | head -1 | cut -d: -f1)
rline=$(grep -n "RESET .*reset .*$H/.cache/omarchyvr\$" <<<"$out" | head -1 | cut -d: -f1)
bkline=$(grep -n "RESET .*backup " <<<"$out" | head -1 | cut -d: -f1)
if [[ -n $bline ]]; then ok "the build cache is listed as a build cache to remove"; else bad "build cache not listed: $out"; fi
if [[ -n $bline && -n $bkline && $bline -lt $bkline ]]; then ok "it goes before the settings backup (never inside the tar)"; else bad "order: build=$bline backup=$bkline"; fi
if [[ -n $rline ]]; then ok "the rest of ~/.cache/omarchyvr is still reset with the settings"; else bad "cache dir not reset"; fi
if grep -q "build cache; not part of the settings backup" <<<"$out"; then ok "the line says why"; else bad "no reason on the line"; fi
[[ -f $H/.cache/omarchyvr/build/cargo/release/libbig.a ]] && ok "dry run removed nothing" || bad "dry run removed the cache"
# and the real run removes it before the tar is made (order in the script)
if awk '/do_rm BUILD "\$BUILD_CACHE"/ { b = NR } /tar -czf "\$SETTINGS_TAR"/ { t = NR } END { exit !(b && t && b < t) }' "$HERE/uninstall.sh"; then
	ok "uninstall.sh removes the build cache before it writes the settings tar"
else bad "order in uninstall.sh"; fi

echo
if [ "$fails" = 0 ]; then echo "all checks passed"; else echo "$fails check(s) FAILED"; fi
[ "$fails" = 0 ]
