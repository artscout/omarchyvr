#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Test: the uninstaller keeps the headset's factory calibration (device data, like ai.toml), also from the older
# ~/.config/pimax place, and says so in its dry run. Fake HOME, dry run only: nothing is changed, no root, no USB.
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
fails=0
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=$((fails + 1)); }
T=$(mktemp -d "${TMPDIR:-/tmp}/uninstall-calib-test.XXXXXX")
trap 'rm -rf "$T"' EXIT

run() { # run HOME: the dry run with that HOME
	HOME=$1 XDG_CONFIG_HOME= "$HERE/uninstall.sh" --dry-run --no-root 2>&1
}
mk() { mkdir -p "$(dirname "$1")"; printf '<?xml version="1.0"?><Calibration/>\n' >"$1"; }

H1=$T/h1
mk "$H1/.config/omarchyvr/headset-calibration.xml"
echo "k=v" >"$H1/.config/omarchyvr/settings.toml"
out=$(run "$H1")
grep -q 'keep .*headset-calibration.xml' <<<"$out" && ok "current place: listed as kept" || bad "current place not kept: $out"
grep -q "reset .*$H1/.config/omarchyvr" <<<"$out" && ok "the rest of the settings still reset" || bad "settings not reset"
[[ -f $H1/.config/omarchyvr/headset-calibration.xml ]] && ok "dry run changed nothing" || bad "dry run removed the file"

H2=$T/h2
mk "$H2/.config/pimax/headset-calibration.xml"
mkdir -p "$H2/.config/omarchyvr"
echo "k=v" >"$H2/.config/omarchyvr/settings.toml"
out=$(run "$H2")
grep -q 'keep .*headset-calibration.xml' <<<"$out" && ok "older ~/.config/pimax place: kept (moves to the new place)" || bad "older place not kept: $out"

H3=$T/h3
mkdir -p "$H3/.config/omarchyvr"
echo "k=v" >"$H3/.config/omarchyvr/settings.toml"
out=$(run "$H3")
if grep -q 'headset-calibration.xml' <<<"$out"; then bad "no calibration, but a keep line appeared"; else ok "no calibration: no keep line"; fi

echo
if [[ $fails == 0 ]]; then echo "all passed"; else echo "$fails failed"; exit 1; fi
