# Controllers on Linux: attach logic, decoder, replay test

Files: `linux/pimax_controllers.py` (reader), `linux/test_controllers_replay.py` (offline test, 0 failures; needs tshark for the
captures2 part, `analysis/p82.pkl` for the right-controller part). Run: `python3 linux/test_controllers_replay.py`.

## What the host does before EP 0x82 reports start (captures2/130457 cold-start, captures/124629)
Cold start (pcap-relative t, from `an` scan of dev 26): wake `1a/5 [01]` 52.80; keepalive `39/0` from 52.88; panel-on request
`29/0 [01]` 58.05; `1a/5 [02 48]` (72 Hz) 58.27; DP scanout ~60.5; `fps_stable_hit_cnt>10` 66.0; `DISPLAY_STATE_WORK_ON` 66.62;
`Panel ON` 67.205; slot 2 burst 67.271 (66 ms after Panel ON); slot 3 burst 69.288 (right after the 0x55/0x5b ept upload at 69.0).
So Windows attaches AFTER the panel is on, one burst per slot, never repeated; but the burst is not strictly gated by the panel:
- Round 1 (124629, headset already up): burst follows `0x04 StartPair` success by 30-40 ms (slot 3 t=71.836, slot 2 t=122.189).
- Replies when the controller is awake, in order: `50` +3 ms (headset-local, `f4 01 a0 a0 8f 10 10 86`), `16` +10-12 ms
  (serial `H30100X000000000001` L / `H30101X000000000001` R), `0a` +15-20 ms, `03` +20 ms (`00 08 2d 6b` L / `00 08 2e 62` R).
  EP 0x82 reports begin <1 ms after the `16` command (the stream starts on the command, not on the reply).
- Today's failed attempt: only `0x50` answered. 0x16/0x0a/0x03 are relayed over the radio to the controller, so no reply
  means the controller was asleep/off (inference; consistent with the capture without controllers, 124423 idle-tracking: no
  0x37, no 0x82, no burst).

## 0x37 heartbeat
`4c 0f 37 SS 48 <u32> <batt%> 00 02 01 00 08 <fw>`, unsolicited, ~2 s per slot, up to TWO messages per 64 B packet (second at
byte 16, no 0x4c magic; slot 3 first seen ~4 s after slot 2). Findings:
- It is the "controller is awake and radio-linked but NOT attached" announce: present from the first captured packet of the
  cold start (t=20.66) until the burst; the last 0x37 for slot 2 is 0.33 s before its burst, slot 3 0.30 s before its burst.
  After attach it stops (no 0x37 in any capture with live 0x82 data, nor in today's live run).
- Asleep controller: no 0x37 at all (nothing to announce). No capture shows awake->asleep, so a sleeping controller coming back
  is assumed to re-announce with 0x37 (untested; the retry logic below depends on it).
- Shutdown captures: reports just stop, host sends nothing controller-related.

## Attach logic now in pimax_controllers.py
1. One burst per slot at start (`16/1e`, `50`, `0a 00 00 00 10`, `03`, 10 ms apart; nothing else ever sent).
2. Per slot: if a 0x37 for that slot arrives after the last burst and no EP 0x82 report for >1.5 s, re-send the burst
   (rate limited to once per 2.5 s). Also after a re-enumeration reopen.
3. Every 3 s with no live slot it prints: `no controller reports ... PRESS A BUTTON on each controller (or power it on)`.
4. Final line reports bursts sent and which of 0x16/0x50/0x0a/0x03 were answered per slot.

## Decoder (all verified by the replay test unless stated)
Left (slot 2), pcap t vs marker: trigger byte7 (21.8, marker 26.9); A = byte47 bit1 (64.3 / 65.7); B = byte47 bit0 (73.9 / 75.6);
stick click byte47 bit3 (88.55 / 90.1); menu byte48 bit0 (109.1 / 112.0); stick up/down byte6 255/1 (121.3, 136.3), right/left
byte5 ~244/1 (155.3, 161.7); order of first activations equals marker order. Touch (byte 49): bit0 = touch-A, bit1 = touch-B
(note the swapped bit order versus the presses), bit3 = touch-stick (follows the click/deflection), bit4 rests at 1 on slot 2 (CORRECTED 2026-10-15 from the Windows redo take 5: bit 4 is "hand on the controller" on both hands, not a left/right rest state; bits 0 and 1 rise together for either face button; decoder `TouchBits::hand_on`)
and 0 on slot 3 (only deviations are reported as `touch-t4`; on slot 3 it fires ~0.1 s before the grip squeeze at 99.8 s).
Changes made: battery is `byte46 & 0x7f` with bit7 = charging (live today: 0xda = 90 % charging L, 0xe4 = 100 % charging R;
the captures had 0x48 = 72 %, 0x63 = 99 % with bit7 clear); byte 12 is now 1 live (was 0 in captures; unknown, ignored);
touch names added; diff view ignores bytes 28/29/51 (aux ADC / checksum noise).
Open: GRIP. Byte 44 (squeeze bell curve on the right in round 1, 0.2 s snap on the left at 45.96) is kept as grip, but the
left grip marker (44.46) precedes it by 1.5 s while a stray byte-7 pulse (43.46-44.04) sits just before the marker; the test
allows 2 s slack for that one. Right controller (slot 3): trigger (byte7 max 255 at 75.5), grip (byte44 max 173 at 73.0),
stick Y all fit the same bytes; its A/B/stick-click/menu bits (bytes 47/48) were NEVER exercised in any capture (assumed same
as left; the system button, if different, is unknown). Right touch bits in round 1 (0x02, 0x08 near the trigger pull) cannot
be assigned to a physical control. Still wanted: a capture/live run pressing every right-hand button.

## Live check (task 3)
`python3 linux/pimax_controllers.py 20 --raw`: controllers were AWAKE; both slots attached 0.1 s after the first burst, all four
replies arrived for both slots, EP 0x82 reports arrived on IF1 (hidraw4) at ~250 Hz per slot (L 1335, R 1494 in 6 s), no 0x37
(already attached). No buttons were pressed in the test runs, so only idle values were seen. keepalived stayed active.

## Live confirmation (2026-10-13, user pressed buttons on both controllers)
- Right controller (slot 3): A (byte 47 bit1), B (bit0), stick click (bit3), trigger byte 7 (0..255),
  grip byte 44 (0..242 seen), stick bytes 5/6 (up and right positive), touch-A / touch-B / touch-stick
  all CONFIRMED, same layout as the left.
- Menu (byte 48 bit0) confirmed on the left; right menu/system not pressed (may be the system button).
- Touch bit "t4" (byte 49 bit4) switched on during use on the left and stayed on: likely proximity / hand-held
  sensor, not a button.
- Battery: L 90 % (charging bit cleared mid-run when unplugged), R 100 % charging.
