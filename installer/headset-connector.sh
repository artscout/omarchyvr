#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# shellcheck shell=bash
# headset_connector, shared by install.sh (via lib.sh) and the root job (sourced, never run). OVR_DRM_ROOT is for tests.

# The headset's display connector, from the EDID manufacturer id "PVR". Prints "<pci> <drm connector> <nvkms name>".
# nvidia-drm names connectors per type from 1 (DP-1), NVKMS from 0 (DP-0): the override wants the NVKMS name.
headset_connector() {
	local e c card pci mfg name t n drm=${OVR_DRM_ROOT:-/sys/class/drm}
	for e in "$drm"/card*-*/edid; do
		[[ -n "$(head -c 8 "$e" 2>/dev/null | od -An -t x1)" ]] || continue
		mfg=$(od -An -t x1 -j8 -N2 "$e" | tr -d ' \n')
		[[ "$mfg" == 42d2 ]] || continue # "PVR" (EISA id bytes 42 d2)
		c=${e%/edid}
		c=${c##*/}
		card=${c%%-*}
		name=${c#*-}
		pci=$(basename "$(readlink -f "$drm/$card/device")")
		t=${name%-*}
		n=${name##*-}
		printf '%s %s %s-%s\n' "$pci" "$name" "$t" "$((n - 1))"
		return 0
	done
	return 1
}
