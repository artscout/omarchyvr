#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Test of the headset detection of install.sh (lib.sh headset_state) on a fake /sys/bus/usb/devices: no USB, no root.
#   awake (34a4:0044), asleep (0043 + 0014, 0043 + 2104:0561), only 0014 -> not the tested headset, nothing.
# Same test vectors as rust/omarchyvr-gui-core/src/install.rs (headset_state_from_usb).
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
. "$HERE/lib.sh"
# shellcheck source=components.lock
. "$HERE/components.lock"
fails=0
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=$((fails + 1)); }
T=$(mktemp -d "${TMPDIR:-/tmp}/headset-detect-test.XXXXXX")
trap 'rm -rf "$T"' EXIT
export OVR_USB_ROOT=$T/usb
check() { # check EXPECTED "vid:pid ..."
	local want=$1 n=0 id
	rm -rf "$T/usb"
	mkdir -p "$T/usb"
	for id in $2; do
		mkdir -p "$T/usb/1-$n"
		echo "${id%%:*}" >"$T/usb/1-$n/idVendor"
		echo "${id##*:}" >"$T/usb/1-$n/idProduct"
		n=$((n + 1))
	done
	local got
	got=$(headset_state)
	[[ $got == "$want" ]] && ok "[$2] -> $want" || bad "[$2] -> $got, wanted $want"
}
check tested "1d6b:0003 34a4:0043 34a4:0044 34a4:0014 2104:0561"
check tested "34a4:0044"
check asleep "34a4:0043 34a4:0014 2104:0561"
check asleep "34a4:0043 34a4:0014"
check asleep "34a4:0043 2104:0561"
check untested "34a4:0014"
check untested "34a4:0043"
check absent "2104:0561"
check absent "1d6b:0003"
check absent ""
if [[ $fails == 0 ]]; then echo "all headset detection tests passed"; else echo "$fails failed"; exit 1; fi
