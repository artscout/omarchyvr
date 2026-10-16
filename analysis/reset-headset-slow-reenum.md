# reset-headset: why Linux sees the headset again after 61-80 s (Windows: 7.8 s)

Live run 2026-10-16 11:11 (`omarchyvr vr reset-headset`, mode sys, `58 04 28 00 00`, reply `4c 04 28 00 01 07`).

Evidence (kernel log, `journalctl -k`):
* 11:11:14 disconnect of 1-5.1.4 (headset hub 05e3:0610), 1-5.1.4.1 (HID), .3, .3.3 (0043), 2-4 and 2-4.4 (camera): 1.1-1.3 s after the command, same as Windows.
* Nothing at all until 11:12:15 (61 s): `usb 1-5.1.4: new high-speed USB device number 13`, no "New USB device found" and no error line (silent failed enumeration, address 14 also consumed).
* 11:12:33 (79 s): device 15 enumerates fine, everything follows within 1.5 s. Windows take 2: command 8.72, off USB 9.75, back 16.56 (7.8 s).
* keepalived was stopped before the command (`omarchyvr-keepalived is inactive`) and restarted only after the camera was back. Windows kept sending `39 00 5a` every 0.22 s and the `19` frames up to the moment the device left USB (9.73 s), and nothing to any hub or other interface afterwards (only the keyboard/BT adapter traffic on other devices; control transfers to hubs 2/3 are plain status polls, no port power control). So after the device is off the bus Windows does nothing that Linux does not.
* Host side difference that remains: `power/control=auto, autosuspend_delay_ms=0` on hubs 1-5 (05e3:0608), 1-5.1 (1a40:0101, the hub the headset hangs from), 2-4 (05e3:0620) and 1-5.1.4. A hub with delay 0 suspends the instant its only child is gone; the same hub (1a40:0101, FE1.1-class) then has to notice the new connect from suspend. The HID/bridge devices already sit at `on`. The first (silent, failed) attempt at 61 s and the good one 18 s later fit a hub that wakes late.

Status: strongest candidate, not proven (no root, live session running, no restart test done). Fix prepared, not applied:
`linux/70-pimax.rules` now also sets `power/control=on` for 1a40:0101, 05e3:0608/0610/0620 and 34a4:*. It installs through the existing app Fix (pkexec) path (omarchyvr-doctor / checklist install `70-pimax.rules`); after install, `udevadm trigger --subsystem-match=usb` (or the next replug) applies it.

Next test (between sessions, after the rule is installed): `cat /sys/bus/usb/devices/1-5.1/power/control` must say `on`, then `omarchyvr vr reset-headset`; expect HID back in about 8 s. If it is still 60+ s, the delay is inside the headset (a ~60 s boot wait), and the next suspect is the headset waiting for the host heartbeat: keep `omarchyvr-keepalived` running and let the tool send `39 00 5a` itself until the HID node disappears (as Windows did).
