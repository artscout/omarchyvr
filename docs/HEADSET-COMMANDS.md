# Headset commands taken from the Windows captures (2026-10-15)

All bytes are from `analysis/windows-capture-20261015.md` (first session and the redo) and are in the HID allowlist
`rust/pimax-device/devices/dream-air/device.toml` (`[[hid]]`). The user approved them on 2026-10-15. Nothing else is sent.

| Allowlist id | Bytes (IF0, zero padded to 64) | Reply | Used by |
|---|---|---|---|
| `haptic` | `58 09 07 SS AMP DUR_lo DUR_hi 00 00 00` (SS 2/3, AMP 1-100 %, DUR ms) | none | Monado haptics, `pimax controllers buzz`, app Test vibration |
| `unpair` | `58 03 05 SS` (SS 2 left, 3 right) | `4c 04 05 SS 01 07` | Unpair buttons (pimax-autod) |
| `ipd_motor` | `58 06 5c 05 01 <u16 LE mm x100>` (58.00 mm = `a8 16`) | `4c 04 5c 05 01 07` | Optics > Apply (`pimax-eye ipd --set MM`) |
| `sys_reset` | `58 04 28 00 00` (eSpiCmdSysReset) | `4c 04 28 00 01 07` | Restart headset (`pimax vr reset-headset`) |

## IPD (Optics)
Apply saves `~/.config/pimax/ipd.env` (what Monado draws with from the next start) and, when no VR session runs, moves
the lenses (`pimax-eye ipd --set MM`: one write, clamped to 58-72 mm, then the live lens IPD is read back from the info
report `53 31`, bytes 45-46; the motor moves about 1.8 mm/s). The range is the one the command was approved for (the
Windows capture reached 58.00 mm; Pimax's own limits were not logged). Windows also sends the eye tracker a lens update
on every step; we do not (the eye tracker is never touched here). While a session runs the page greys the move and says
why; there is no path through the running session (Monado) for the lens motor, only the saved value applies at the
next start. The page shows "Lenses now" from the headset (read once when the row opens and after each move).

## Restart headset
`58 04 28 00 00` makes the headset leave USB after about 1 s, come back (HID) after about 8 s, the camera after about 17 s
and show a picture after about 22 s. `pimax vr reset-headset` (default `--mode sys`) stops pimax-keepalived, sends it
once, prints `restart: ...` stage lines (the app shows them in plain words on a ~30 s bar), waits until 22 s after the
command and restarts pimax-keepalived. If nothing leaves USB within 15 s the headset ignored the command and the old
exit sequence (`--mode exit`, heartbeat timeout) follows, then the dp retry if the camera is still missing. Refused while
monado-service or a session runs. After a restart Windows sends no panel-on `58 04 29 00 01` and the panel comes on by
itself; our session still sends it in the normal init (harmless).

First wake after a restart: ignored by the firmware. The session (`pimax::Session`) re-sends `58 06 1a 05 01 00 00` once
if no `4c .. 1a` reply arrived within 0.5 s (`WAKE_RESEND_AFTER`), as Pimax Play does.

## Controller touch byte (report byte 49)
Bit 0x10 (`TouchBits::hand_on`) is "hand on the controller" on both hands. Bits 0x01 and 0x02 rise together for either
face button (`TouchBits::thumb()`); they are not per-button touches. Monado's Pimax controller driver uses bits 0/1/3 only
(thumb near the buttons, stick), never bit 4, so no Monado rebuild was needed for this fix.
