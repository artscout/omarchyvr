#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Test of the udev step of the root job (no root, no udev): the trigger after installing the rule covers every device
# the rule covers, the eye tracker 2104:0561 too. Real install 2026-10-17 12:40: the job triggered only 34a4:* and
# hidraw, the eye chip's node stayed root-only, the app's access check failed and a second password was asked.
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT_SH=$HERE/omarchyvr-root.sh
fails=0
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=$((fails + 1)); }
T=$(mktemp -d "${TMPDIR:-/tmp}/root-udev-test.XXXXXX")
trap 'rm -rf "$T"' EXIT

line=$(grep -m1 "^UDEV_TRIGGER=" "$ROOT_SH")
[[ -n $line ]] || bad "UDEV_TRIGGER not found in omarchyvr-root.sh"
eval "$line"
mkdir -p "$T/bin"
printf '#!/bin/sh\necho "udevadm $*" >>"%s/calls"\n' "$T" >"$T/bin/udevadm"
chmod +x "$T/bin/udevadm"
PATH=$T/bin:$PATH sh -c "$UDEV_TRIGGER"
calls=$(cat "$T/calls" 2>/dev/null)
grep -qx "udevadm control --reload" <<<"$calls" && ok "reloads the rules first" || bad "no reload: $calls"
grep -q "trigger --subsystem-match=usb --attr-match=idVendor=34a4" <<<"$calls" && ok "triggers the headset's USB devices" || bad "34a4: $calls"
grep -q "trigger --subsystem-match=usb --attr-match=idVendor=2104 --attr-match=idProduct=0561" <<<"$calls" && ok "triggers the eye tracker 2104:0561" || bad "eye chip not triggered: $calls"
grep -q "trigger --subsystem-match=hidraw" <<<"$calls" && ok "triggers the hidraw nodes" || bad "hidraw: $calls"
# every vendor/product the rule grants access to is triggered
for id in $(grep -o 'ATTRS{idVendor}=="[0-9a-f]*"' "$HERE/../linux/70-pimax.rules" | grep -o '[0-9a-f]\{4\}' | sort -u); do
	grep -q "idVendor=$id" <<<"$calls" && ok "rule vendor $id is triggered" || bad "rule vendor $id is never triggered"
done
# the apply step uses it (and only after the rule file changed)
if grep -q 'act "udevadm control --reload; udevadm trigger for the Pimax devices and the eye tracker" -- sh -c "$UDEV_TRIGGER"' "$ROOT_SH"; then
	ok "the udev step runs this trigger"
else bad "the udev step does not use UDEV_TRIGGER"; fi

echo
if [ "$fails" = 0 ]; then echo "all checks passed"; else echo "$fails check(s) FAILED"; fi
[ "$fails" = 0 ]
