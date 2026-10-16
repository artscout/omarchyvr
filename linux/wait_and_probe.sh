#!/bin/bash
# Wait for the headset (34a4:0044) to enumerate, then immediately run the probe.
# Usage: sudo ./wait_and_probe.sh [seconds]
cd "$(dirname "$0")"
echo "waiting for 34a4:0044 ..."
until ls /sys/class/hidraw/*/device/uevent 2>/dev/null | xargs grep -l 'HID_ID=0003:000034A4:00000044' 2>/dev/null | grep -q .; do sleep 0.05; done
sleep 0.3   # let all 4 interfaces bind
echo "headset up at $(date +%T.%N)"
python3 pimax_probe.py "${1:-30}"
echo "--- kernel log during run ---"
journalctl -k --since "-$(( ${1:-30} + 5 )) s" --no-pager | grep -E '1-5.2.4.1|0044' | tail -20
