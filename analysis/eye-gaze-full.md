# Tobii TTP gaze notification (1242 B), fully decoded - 20261013-130650-full-packets-hub2.pcap
Dev 30, EP 0x83 bulk IN: 364 packets in 3.0 s (**120.49 Hz**), all 1242 B, nothing truncated. Tools: `tools/ttp_gaze.py`.
**No eye is tracked in this capture (headset not worn)**: every gaze/pupil field holds an invalid sentinel. Value semantics are therefore from structure plus the earlier truncated captures (idle-tracking, controllers) where the first eye group was valid. Second-eye semantics are structural inference only.

## Framing
```
off 0   u32 LE 1            wrapper: 1 = IN
off 4   u32 LE 0x4da=1242   wrapper: total length
off 8   u32 BE 0x53         TTP magic (notification)
off 12  u32 BE 0            seq (always 0)
off 16  u32 BE 0
off 20  u32 BE 0x0505       stream/subscription id (matches 0x4ce(0x0505) subscribe)
off 24  u32 BE 0
off 28  u32 BE 0x4ba=1210   payload size (NOT an opcode - corrects eyetracking.md; 1242-32)
off 32  u16 0000            pad
off 34  TLVs: tag u8 | len u32 BE | value  - 132 items, end exactly at 1242
```
Tags: 2 = int32, 3 = S15.16 (int32/65536), 4 = 8-byte value (only the sentinel ff ff fc 00 00 00 00 00 seen), 5 = u32 id, 6 = int64 us.
Item 0 = tag5 0x230bb8 (payload type). Then fields: each starts with `tag5 0x20bb9` + `tag2 <field no>` followed by value items. Value type ids: 0x31f42 = vec3 (3 x S15.16), 0x21f43 = vec2 S15.16, 0x21f40 = 2 x tag4. Field order is fixed in all 364 packets:
`1,2,3,4,5,6,7,16,23,22,30,31 | 8,9,10,11,12,13,17,25,24,32,33 | 14,15,18,19,20,21,26,27,29,28,34,35`

## Field table ("this capture" = invalid; "valid" = seen with eyes tracked in snaplen-512 captures)
| field | type | this capture | meaning (confidence) |
|---|---|---|---|
| 1 | int64 us | 50 376 465, +8300 every packet | tracker timestamp; same crystal as the IMU clock, offset IMU-gaze = 44 171 5xx us here (drift 0.2 ppm, jitter ~1 ms from USB), offset is per boot so must be measured at runtime (high) |
| 2 | vec3 mm | 0,0,0 | eye A gaze origin (valid e.g. 30.8,-4.6,-27.4) (high) |
| 3 | int | 0 | validity of 2, 1 = valid (high) |
| 4 | vec3 | 0,0,0 | eye A gaze direction, unit (valid e.g. 0.32,0.14,0.94) (high) |
| 5 | int | 0 | validity of 4 (differs from 3 in ~6 % of valid-era frames) (high) |
| 6 | S15.16 | -1.0 | eye A pupil diameter mm (valid 2.6-4.6) (high) |
| 7 | S15.16 | -1.0 | eye A second scalar, -1 even in valid frames: eye openness, apparently unsupported (low) |
| 16 | 2 x tag4 | sentinel | eye A extra pair, never valid (low) |
| 23 | vec2 | -1,-1 | eye A pupil position in sensor area, normalized 0..1 (valid 0.45,0.57) (med) |
| 22 | int | 0 | validity of 23 (med) |
| 30 / 31 | int | 1 / 0 | flags (low) |
| 8 | vec3 | 0,0,0 | eye B gaze origin, twin of 2 (med) |
| 9 | int | 0 | validity of 8 |
| 10 | vec3 | 0,0,0 | eye B gaze direction |
| 11 | int | 0 | validity of 10 |
| 12 / 13 | S15.16 | -1 / -1 | eye B pupil diameter / openness-like |
| 17 | 2 x tag4 | sentinel | twin of 16 |
| 25 | vec2 | -1,-1 | eye B pupil position in sensor area |
| 24 | int | 0 | validity of 25 |
| 32 / 33 | int | 1 / 0 | flags (twin of 30/31) |
| 14 | int | 1 048 575 (0xFFFFF) | constant, unknown (low) |
| 15 | int | 5412.. +1 per packet | **frame counter** (high) |
| 18 / 20 | vec3 | -1,-1,-1 | combined/aux 3D points (gaze point / convergence?) invalid (low) |
| 19 / 21 | int | 0 | validities for 18 / 20 |
| 26, 27, 28 | int | 0 | flags (low) |
| 29 | S15.16 | -1.0 | convergence distance (guess) (low) |
| 34 | S15.16 | 57.3536 const | unknown constant (low) |
| 35 | int | 1 | const |

Eye groups: eye A = fields 2-7,16,23,22,30,31; eye B = 8-13,17,25,24,32,33; the rest is frame-level. In earlier truncated captures (first 48 TLVs = eye A only, ends inside eye B) valid data appeared only in eye A. Which physical eye A is: origin x = +31 mm; right eye if x points to the user's right (unverified - test by occluding one eye).
Validity pair (3,5) in idle capture: (1,1) 54 %, (0,0) 40 %, (1,0) 6 %.

## Other full-size payloads
* No EP 0x84 log lines and no TTP command traffic in this window (dev 30 only streams). Headset EP 0x01/0x81 packets are 64 B (never truncated): `58 19 51 00 01 32 <exposure u16 x4> <gain u16 x4> 28 00 3e 00` at 12.5 Hz, reply `4c 04 51 00 01 07`; the same exposure/gain words appear in the camera frame metadata (see camera-frames.md), so the 0x58 0x19 command is the SLAM auto-exposure/gain update.
* Strings: dev 26 = "Pimax Dream Air" (Pimax Corporation), serial P511X0X000000000001; dev 28 = "Pimax Device"; dev 3 = MSI Mystic Light (unrelated).
