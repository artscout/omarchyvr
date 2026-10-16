#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Run by udev (linux/70-pimax.rules) when a Pimax USB device (34a4:*) appears: keep the USB hubs ABOVE it out of
# runtime autosuspend. Only the hubs on the headset's own chain are touched, never other hubs.
# Why: after a headset reset it leaves USB for a moment; a hub with power/control=auto and autosuspend delay 0
# suspends as soon as its only child is gone, and the headset then took 61-80 s to come back (Windows: 7.8 s).
# $1 = the device path from udev (%p), e.g. /devices/pci0000:00/.../3-2.1
set -eu
p="/sys${1:?devpath}"
while :; do
	p=${p%/*}
	case $p in /sys/devices | /sys/devices/* ) ;; *) break ;; esac
	case ${p##*/} in usb[0-9]*) break ;; esac # stop below the root hub (the controller)
	[ "$p" = /sys/devices ] && break
	if [ -r "$p/bDeviceClass" ] && [ "$(cat "$p/bDeviceClass")" = 09 ] && [ -w "$p/power/control" ]; then
		echo on >"$p/power/control"
	fi
done
