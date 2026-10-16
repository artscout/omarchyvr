#!/bin/bash
# MVP: light the Dream Air panel on Linux (NVIDIA, Hyprland). Run as the desktop user; uses sudo for debugfs/sysfs.
# Needs: omarchyvr-keepalived running. Mode: 7104x1840@72 (990 MHz, 24 bpp, 4 DP lanes, no DSC).
# Why the EDID override: the stock EDID only lists 7104x3840 @ 2.17/2.64 GHz (nvidia-drm rejects them, no modes),
# and declares colour depth "undefined" so nvidia sends 18 bpp, which the LT7911UXE bridge does not turn into a picture.
set -e
cd "$(dirname "$0")"
EDID=../analysis/dp-edid-tall72-8bpc.bin
CONN=card1-DP-1
sudo sh -c "cat $EDID > /sys/kernel/debug/dri/1/DP-1/edid_override; echo off > /sys/class/drm/$CONN/status"
sleep 2
sudo sh -c "echo detect > /sys/class/drm/$CONN/status"
sleep 1
echo "modes: $(tr '\n' ' ' < /sys/class/drm/$CONN/modes)"
hyprctl eval 'hl.monitor({ output = "DP-1", mode = "7104x1840@72", position = "1920x0", scale = 1 })' >/dev/null
hyprctl reload >/dev/null
sleep 2
hyprctl monitors all -j | python3 -c "import json,sys;[print('hyprland:',m['name'],m['width'],'x',m['height'],'@',m['refreshRate']) for m in json.load(sys.stdin) if m['name']=='DP-1']"
# announce 72 Hz video + panel-on request, then show the bridge log (Panel ON comes ~5.5 s after stable video)
python3 -u pimax_video_on.py 72 "${1:-20}" | grep -E 'bridge|hmd' | grep -iE 'mipi|panel|WORK_|bl_bright|aw883xx_start:|fps calc 72.*stable/un (1|11|20)/'
