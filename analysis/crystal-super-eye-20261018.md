# Crystal Super eye tracking: Windows takes of 2026-10-18 (wear, fit, slip, calibrate, dfr)

Taken with `windows/`-style scripts on the shared drive (`EYE-ALL.cmd`: USBPcap on every hub with timed, marked steps,
plus read-only copies of the Tobii and Pimax files before and after). Client: Pimax EVO / PimaxClient 2.0.2.103, Tobii
VR4PIMAXP3B Platform Runtime 1.21.0.0. Raw files stay off the repo (serials). Analysis:
`analysis/tools/crystal_eye_takes.py` (gaze, eye origins and headset rotation per marked step) and
`rust/pimax-eye/examples/op_dump.rs` (payloads of one opcode, reassembled).

## What Pimax does while the headset is worn, taken off, put on, moved

- **wear** (off, in the hands, on, off, on, close the client): **no request to the eye chip** at all; the gaze stream
  goes on by itself and the eyes come back when the headset is put on. The only request is the unsubscribe (0x4ce)
  when the client closes. (The full chip restart of take `sleep-wake` of 2026-10-17 followed the client's own standby;
  without it the chip keeps running.)
- **fit** (high, low, tilted, loose) and **slip** (one point watched while the headset slides up and left): **no
  request** either. Whatever compensates the fit runs inside the chip.
- Valid samples with the headset worn: about 96 % (fit). Our own Linux session (raw log of the pimax-eyecal run of
  17:58) has 84-85 % valid, in blocks (blinks), not alternating: the chip runs normally under Linux.
- No wear reminder appeared in any step; this client has no switch for it (the program `EyeTrackingGuide` is installed).

## How much the gaze moves when the headset moves on the face

`crystal_eye_takes.py` on fit and slip: per still step the eye origins (chip frame, mm), the gaze and the headset
rotation from the gyro (axes from the data: yaw = gyro axis 1, r 0.88; pitch = axis 0, r 0.80; the gyro's slow drift
taken out with a line through the steps where the headset sat in its place). The user watched one point that is still
in the world, so the gaze in the headset frame must move by minus the headset rotation; the rest is the tracker's error.

| step (against the first) | eye origin change (mm) | error left, eye A / eye B (deg) |
|---|---|---|
| slip: headset up | y -8, z -4 | 4.6 / 1.1 |
| slip: headset left | x -6..-8, z -4 / -17 | 1.2 / 1.6 |
| fit: high | y -9..-11, z -3 | 3.0 / 2.4 |
| fit: low | y +4..+5, z -2 | 4.0 / 1.8 |
| fit: tilted | y +2.5 / -2.7 | 1.7 / 1.1 |
| fit: loose (away from the face) | z -12 | 1.6 / 2.3 |
| back in place (4 steps) | under 2 | 0.2-0.9 |

Mean error of the moved steps 1.8 deg (eye A) and 1.3 deg (eye B). A linear correction by the origin change
(least squares over all steps) only brings it to about 1.0 deg; the one clear term is eye A's pitch, -0.32 deg per mm of
vertical origin change. So **the chip compensates the fit itself**, to 1-2 degrees, with Pimax's calibration loaded.

## The calibration is the chip's own (Tobii), not the client's

Tobii's log (`ProgramData\Tobii\Tobii Platform Runtime\XR5EYECHIP\pr_log0.txt`) and take **calibrate** (three runs):

| request | what | payload |
|---|---|---|
| 0x3f2 | start the calibration | none |
| 0x407 | one target: x, y, z (mm, z = 2000) and the eyes | 50 bytes; **the same bytes for the same target** (9 targets, each 3 times, 27 requests) |
| 0x42e | compute (Tobii log: "computed calibration for both eyes", "Committed new calibration, size 37401") | none |
| 0x44c | download the profile | answer 32-46 KB |
| 0x3fc | finish | none |

Targets of the two sets Pimax uses (mm at 2 m): centre and the diagonals (+-536, +-536) (15 deg); and (+-932, 0),
(0, 840), (0, -932) (25 / 23 deg). The downloaded profile **is** `calibration.setpm`: the file is a 16-byte header and
the same 37 401-byte body the answer carries after its 11-byte TLV prefix (`00 00 | 15 | len+4 | len | body`). The
upload at every chip start (0x456) has exactly the download's layout. The client stores the file and uploads it at the
next start; no upload happened in these takes (the chip was not restarted).

**Our start list uploads an old profile**: `eye-start-crystal-super.txt` comes from take cold2 of 2026-10-17 21:55, before
the calibrations of 17 Oct 22:20 and 18 Oct 20:47 (30 875-byte body; today's is 37 401). The chip's own model then fits
another calibration: our raw error was 8-11 degrees and the gaze moved 3-6 degrees when the headset was put on again,
while Windows, with the current profile, stays within 1-2.

## Foveated rendering under Pimax (take dfr, Falcon BMS)

The gaze stream stays the consumer stream (0x0505); the Tobii log shows the runtime subscribing its
`WEARABLE_FOVEATED` stream for foveated apps (without its own chip request in the take). Pimax's quad views settings
(`AppData\Roaming\Pimax\AppConfig\global.json`, `Local\Pimax\runtime\profile.json`): periphery resolution scale
0.0909, gaze (focus) resolution scale 0.5, focus blend area 0.05, focus transition opacity 0.5 (the edge never goes
below half the focus, as in Quad-Views-Foveated), rendering level 5, fine FOV 0.33 (the client shows 33 % but renders
about 67 %: an inverted field after the update, per the user).

## Ways to get the current calibration into the chip under Linux

1. **Capture the next cold start** of the client under Windows (2 minutes: close it, record, start it): its start list
   carries the current profile upload byte for byte. Fully within "bytes only from Windows captures"; must be repeated
   after every calibration in Windows.
2. **Build the 0x456 upload from the user's `calibration.setpm`** (its body in the download's layout, verified above):
   no Windows trip after a calibration, but the upload message is assembled, not captured.
3. **Calibrate the chip from Linux** with the captured requests of take calibrate (0x3f2, the nine 0x407, 0x42e, 0x44c,
   0x3fc, all byte-identical across runs), showing the same targets; keep the downloaded profile and upload it at every
   start. Needs the calibration opcodes (refused by `pimax-eye`'s Guard today) to be allowed.

## Result (option 2, 18 Oct 22:00-22:12)

`pimax-eye profile calibration.setpm -o ~/.config/omarchyvr/eye-profile-crystal-super.bin` takes the body out of the
file (header and the body's own length field checked); `frames::with_profile` puts it in place of every 0x456 step of
the start list (the Crystal Super's start uploads the profile twice, seq 20 and 22), with the captured header and the
Windows split into 4095-byte transfers. The two captured uploads are rebuilt byte for byte from their own bodies.
`omarchyvr eyed --profile FILE` / `PIMAX_EYE_PROFILE` upload it at every start; the chip accepts it.

In the headset: the gaze stays put when the headset slides on the face (as under Windows). What was left was a constant
offset between the chip's frame and SteamVR's view, mostly vertical and different per eye (9.1 deg left, 13.7 deg
right); a host calibration with an identity map and only the drift offsets (`pimax-eyecal --recenter`, 5 targets)
removes it. The host correction fitted on the old profile (affine, gains 0.6-0.85) is not used any more.
