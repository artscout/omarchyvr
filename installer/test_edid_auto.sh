#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Test of the headset connector search of the root job (OVR_EDID=auto) on a fake /sys/class/drm: no root, no USB.
#   key found at once, key found after a delay (the headset wakes), never found (timeout, clean answer), a non-Pimax
#   monitor is ignored, the plan text of the auto step, and an explicit key keeps its old plan line.
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT_SH=$HERE/omarchyvr-root.sh
fails=0
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=$((fails + 1)); }
T=$(mktemp -d "${TMPDIR:-/tmp}/edid-auto-test.XXXXXX")
trap 'rm -rf "$T"' EXIT
export OVR_DRM_ROOT=$T/drm OVR_EDID_POLL=0.1
mkdir -p "$T/drm/card1" "$T/pci/0000:01:00.0"
ln -s "$T/pci/0000:01:00.0" "$T/drm/card1/device"
edid() { # edid CONNECTOR MFG_HEX(2 bytes): a 16-byte stub with the manufacturer id at bytes 8-9
	mkdir -p "$T/drm/card1-$1"
	printf '\x00\xff\xff\xff\xff\xff\xff\x00' >"$T/drm/card1-$1/edid"
	printf "\\x${2:0:2}\\x${2:2:2}\\x00\\x00\\x00\\x00\\x00\\x00" >>"$T/drm/card1-$1/edid"
}
key() { "$ROOT_SH" edid-key "$1" 2>&1; }

# a normal monitor (other id) and a disconnected port (empty EDID file) are not the headset
mkdir -p "$T/drm/card1-DP-1"
: >"$T/drm/card1-DP-1/edid"
edid HDMI-A-1 1e6d
s=$SECONDS
out=$(key 2)
rc=$?
if [ $rc -ne 0 ] && [ -z "$out" ] && [ $((SECONDS - s)) -le 4 ]; then ok "no Pimax EDID: fails after the timeout (2 s), prints nothing"; else bad "no headset: rc=$rc out=$out"; fi

# found at once: DP-2 in the kernel's numbering -> NVKMS DP-1
edid DP-2 42d2
out=$(key 5)
if [ "$out" = "override.[0000:01:00.0].DP-1" ]; then ok "found at once: $out"; else bad "immediate: $out"; fi

# appears after a delay (the headset wakes while the job waits)
rm -rf "$T/drm/card1-DP-2"
(
	sleep 1
	edid DP-1 42d2
) &
bg=$!
s=$SECONDS
out=$(key 10)
wait "$bg"
if [ "$out" = "override.[0000:01:00.0].DP-0" ] && [ $((SECONDS - s)) -le 6 ]; then ok "appears after a delay: $out"; else bad "delayed: $out"; fi

# the plan text
DATA=$T/data
mkdir -p "$DATA"
for d in linux patches installer; do ln -s "$HERE/../$d" "$DATA/$d"; done
plan() { # plan EDIDVALUE
	cat >"$T/plan.env" <<EOP
OVR_USER=nobody
OVR_HOME=$T/home
OVR_DATA=$DATA
OVR_NVIDIA=patch
OVR_NVIDIA_VERSION=999.1
OVR_NVIDIA_TESTED=610.57.04
OVR_NVIDIA_ALLOW_UNTESTED=no
OVR_SNAPSHOT=no
OVR_EDID=$1
EOP
	"$ROOT_SH" plan "$T/plan.env" >"$T/out.txt" 2>&1
}
if plan auto && grep -q "EDID override (connector found after the headset wakes, up to 30 s)" "$T/out.txt" && grep -q "if it does not show up the step is skipped" "$T/out.txt"; then ok "plan: the auto step reads plainly"; else bad "plan auto: $(cat "$T/out.txt")"; fi
if plan "override.[0000:01:00.0].DP-0" && grep -q "EDID override for \[0000:01:00.0\].DP-0" "$T/out.txt" && ! grep -q "wakes" "$T/out.txt"; then ok "plan: an explicit key is unchanged"; else bad "plan explicit: $(cat "$T/out.txt")"; fi
if plan "" && ! grep -q "EDID override" "$T/out.txt"; then ok "plan: empty = no EDID step (headset not on USB)"; else bad "plan empty"; fi

# the `edid` mode (the app's one-click Fix): headset display not seen -> exit 3, says so, writes nothing, REBOOT=no
rm -rf "$T/drm/card1-DP-1" "$T/drm/card1-DP-2" "$T/drm/card1-HDMI-A-1"
cat >"$T/plan-edid.env" <<EOP
OVR_USER=nobody
OVR_HOME=$T/home
OVR_DATA=$DATA
OVR_EDID=
EOP
out=$(OVR_TEST_NO_ROOT=1 OVR_EDID_WAIT=1 "$ROOT_SH" edid "$T/plan-edid.env" 2>&1)
rc=$?
if [ $rc -eq 3 ] && echo "$out" | grep -q "EDID override (connector found after the headset wakes" && echo "$out" | grep -q "headset display not seen" && echo "$out" | grep -q "^REBOOT=no"; then
	ok "edid mode: no headset display -> exit 3, plain message, nothing written"
else bad "edid not found: rc=$rc out=$out"; fi
# without the test switch a normal user is refused (the mode needs root)
out=$("$ROOT_SH" edid "$T/plan-edid.env" 2>&1)
rc=$?
if [ $rc -ne 0 ] && echo "$out" | grep -q "must run as root"; then ok "edid mode: refuses to run as a normal user"; else bad "edid as user: rc=$rc out=$out"; fi

echo
if [ "$fails" = 0 ]; then echo "all checks passed"; else echo "$fails check(s) FAILED"; fi
[ "$fails" = 0 ]
