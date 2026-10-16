# Pimax Dream Air - IMU / camera / stream analysis (idle-tracking-hub2.pcap)

Capture: `captures/20261013-124423-idle-tracking-hub2.pcap` (56.64 s, USBPcap, snaplen 512, 81168 frames).
Addresses are per-capture (re-enumerated); device-map.md did not exist, so devices were identified by traffic.

## 1. Periodic streams (this capture)

| Dev addr | EP | Type | Payload | Rate | First bytes | Identity (inferred) |
|---|---|---|---|---|---|---|
| 18 | 0x83 IN | interrupt | 64 B | **500.17 Hz** (28331 reports) | `53 31 08 00` | **Headset IMU** (HID-like, no report ID visible; magic "S1") |
| 18 | 0x01 OUT / 0x81 IN | interrupt | 64 B | 12.5 Hz request/response (708 pairs, 80 ms) + 266 other OUT (~4.7 Hz) | OUT `58 19 51 00 ..`, `58 04 39 00 48..`; IN `4c 04 51 00 01 07` | Host->MCU command/status channel ('X' = cmd, 'L' = reply), 16-bit values changing smoothly; NOT yet decoded |
| 18 | 0x84 IN | interrupt | 64 B | 2 pkts at t=27.35 | ASCII | MCU debug log text: `[00:01:26.051,523] <info> app: <wakeup_dev_check_timer_callback>` (Nordic nRF-style log) |
| 21 | 0x81 IN | **isochronous** | 128 pkts/URB, ~12-18 KB per pkt | 62.5 URB/s, **106.7 MB/s** | payload not captured | **Tracking cameras (UVC)** |
| 22 | 0x83 IN | **bulk** | 1242 B (485 B captured) | **120.5 Hz** | `01 00 00 00 da 04 00 00 00 00 00 53 ..` | Unknown TLV-like stream with BE u32 us timestamp (see 5) |
| 1 | 0x01/0x81 | bulk | 31 B / 13 B | 1 Hz | `55 53 42 43` | USB mass-storage (BOT CBW: TEST UNIT READY) - unrelated |
| 3,4,9,20 | | control / few interrupt | | one-offs | dev 20 EP81: ASCII `99113->[I] etp task`; dev 4: `0e 04 02 39 20 00` x3 | enumeration + misc, dev20 also prints MCU-style text |

Controller dongle (34A4:0043) shows no steady traffic here (controllers not in use). Audio ISO (MI_04) is not in this capture.

## 2. IMU report (dev 18, EP 0x83, 64 bytes, 500 Hz)

### Evidence
* Bytes 0-3 constant `53 31 08 00`; bytes 8-11 constant 0; 45-47 constant `43 18 ca`; 48-63 zero.
* Bytes 4-7 u32 LE: steps +2000 us (18417x) or +1999 us (9913x) -> **microsecond timestamp, 500 Hz**. Span over capture 56 641 583 us vs host 56.640995 s. Starts at 58.71 s. It equals the MCU uptime: the debug log at host t=27.348 prints uptime 00:01:26.051 (86.05 s); 86.05-27.35 = 58.70 s = IMU ts at t=0. So the timestamp is MCU uptime in us (wraps at 2^32 us = 71.6 min).
* Each report holds **two 16-byte samples** (1 kHz sample rate): block 1 lies midway between consecutive block 0 values (|b1-b0| ~ 0.5 x |b0next-b0| for all 6 axes). Block 0 is probably at `ts`, block 1 at `ts+1000 us` (could instead be ts-1000; unverified).
* All sample words are **big-endian int16** (the LE view produces carry-byte jitter; BE gives smooth series).
* Accel = words 1-3: |a| mean over whole capture 2049 LSB, still (t 31-50 s) 2044-2047 -> **2048 LSB/g (+-16 g)**: 0.0047884 m/s^2 per LSB (9.80665/2048). Gravity sits on +Y (ay ~ +2040).
* Gyro = words 4-6: per-second RMS (t=6-14 s, yaw sweeps) dominated by word 5 (gy: 435, 754, 408, 456, 325, 1764 LSB), pitch phase (t=15-20) shows gravity moving from ay to az and large word 4 (gx 686, 1995, 1219). Still noise (t=31-34): ~15-30 LSB RMS per axis (includes small hand/head motion).
* Gyro scale: least-squares fit of d(g_hat)/dt = g_hat x (M * gyro_raw) over the whole capture gives a near-diagonal M = diag(1.00e-3, 1.05e-3, 1.75e-3) rad/s/LSB with off-diagonals < 0.25e-3 -> same axes and same sign as the accelerometer; diagonal x/y imply 16.6-17.4 LSB/dps (z weakly constrained because gravity is nearly in the XY plane; R2 0.51 due to accel noise). Consistent with **16.4 LSB/dps (+-2000 dps)**, 1 LSB = 1.0642e-3 rad/s. Treat as medium confidence; an actual rotation of known angle would confirm.
* Word 0 (BE) 3322 -> 3238 over 56 s, slow monotonic decay, noise +-10: **temperature raw** (unit unknown; the header-less sensor reading, equal in both blocks).
* Tail word (bytes 26-27 of block 0): `e8 03`/`e7 03` = **u16 LE 1000 (sometimes 999)**, block 1 has 0000. Likely the measured sample period in us (hypothesis).

### Proposed struct (little-endian host; sample words are BIG-endian on the wire)
```c
#pragma pack(push,1)
struct pimax_imu_sample {      /* 16 bytes */
    uint8_t  tag;              /* 0x0c (0x0d seen; = high byte of be16 temp, not a real tag) -- see temp */
    uint8_t  temp_lo;          /* together with tag: be16 temp_raw, 3322 -> 3238, slow drift   [conf: med] */
    int16_t  acc[3];           /* BE int16, x,y,z: 2048 LSB/g -> *0.0047884 m/s^2             [conf: high] */
    int16_t  gyr[3];           /* BE int16, x,y,z: 16.4 LSB/dps -> *1.0642e-3 rad/s            [conf: med] */
    uint16_t aux_le;           /* LE, block0 = 1000/999 (period us?), block1 = 0              [conf: low] */
};
struct pimax_imu_report {      /* 64 bytes, EP 0x83 */
    uint8_t  magic[3];         /* 53 31 08 */
    uint8_t  zero0;            /* 00 */
    uint32_t timestamp_us;     /* LE, MCU uptime, +2000/report                                [conf: high] */
    uint32_t zero1;            /* 0 */
    struct pimax_imu_sample s[2];   /* 1 kHz samples, s[0] at ts, s[1] ~ts+1000us (order unverified) */
    uint8_t  const_tail[3];    /* 43 18 ca  (constant; version/CRC?)                          [conf: low] */
    uint8_t  pad[17];          /* 0 */
};
#pragma pack(pop)
/* decode: v = (int16_t)((b[i]<<8)|b[i+1]);   temp = (b[12]<<8)|b[13] */
```
Byte offsets: ts 4; s0 at 12 (temp 12, acc 14/16/18, gyr 20/22/24, aux 26 LE); s1 at 28 (same +16); const at 45.

### Axis mapping (device frame, from gravity + motion)
| Axis | Evidence | Confidence |
|---|---|---|
| acc/gyr Y = world-up when headset level (+1 g reads on +Y) | ay = 2040 at rest | high |
| Yaw (left/right) = rotation about Y = gyr[1] | yaw sweeps at 6-14 s: gy dominant | high |
| Pitch (up/down) = rotation about X = gyr[0] | gx large at 15-20 s with ay<->az exchange | high |
| Roll about Z = gyr[2] | smallest, step at 54-56 s | medium |
| Gyro axes identical to accel axes, positive = right-hand rotation about the axis | LSQ fit diag positive, off-diag small | medium |
| Direction of Z (toward face vs. away) / sign of X relative to user's right | not determinable from this data | unknown |

### Sample values (BE int16 words)
| t (s) | state | acc x,y,z | gyr x,y,z | temp |
|---|---|---|---|---|
| 0.000 | still | -3, 2042, -451 ... | -75, 100, -45 | 3320 |
| 33 | still | 45, 2040, -129 | ~0 +-25 | 3254 |
| 11 | yaw sweep | -48, 2004, -426 | peak gy +-1764 rms | 3290 |
| 18 | pitch | 3, 1471, 126 | gx 1995 rms | 3277 |
Biases: still gyro medians ~ (11, 39, 2) LSB = (0.7, 2.4, 0.1) dps raw (not removed; calibration XML wBias in rad/s = 0.0034, 0.0069, -0.0070 i.e. 0.2, 0.4, -0.4 dps - same order; file has aBias 0.080,-0.022,0.039 m/s^2).
Calibration XML has no IMU rate/noise (IMUNoise all 0, `delta="0.001637992156"` in Stateinit = 1.638 ms, probably IMU-to-camera time offset), so rate 500 Hz/1 kHz comes from the capture only.

## 3. Camera stream (dev 21, EP 0x81 IN, isochronous)
* URBs: 128 iso packets each, USB start-frame +16 per URB at exactly 62.5 URB/s (16 ms): 8 packets/ms (one per microframe). Packet lengths 12-18.9 KB (typ. 15.4-17.2 KB), many 0-length idle packets.
* Total 6 040 435 860 B / 56.6 s = **106.7 MB/s**.
* 12-byte packets (864 seen) end each frame: a UVC payload header only (12 B = BFH length with PTS+SCR), i.e. EOF marker. Observed EOF times lie on a **20 ms grid** (deltas 20/60/80 ms, only 39 of 128 packet descriptors per URB are captured) -> **~50 super-frames/s, ~2.13 MB each** (106.7 MB/s / 50). Pattern: 5 URBs per 4 frames.
* The calibration XML says 4 cameras OV7251 640x480 (trackingA/B, Pi-controllerA/B) with `Image format="interleaved"`, so each frame is very likely the 4 views combined. 4x640x480 8-bit = 1.229 MB does NOT match 2.13 MB (maybe other pixel packing or embedded metadata) - unresolved.
* **UVC payload headers / PTS / SCR / pixels are NOT visible**: USBPcap iso descriptor table (27+12+128x12 = 1575 B) exceeds snaplen 512. No timestamp-to-IMU-clock correlation can be tested from this file. Poweron capture shows only UVC unit controls (GET_INFO/LEN/MIN/MAX/RES/DEF on entity 4, interface 0) and no probe/commit, so streaming is probably started by vendor commands.
* Needed: recapture with snaplen 65535 (or at least > 1575+64, ideally full) for ~2 s.

## 4. Other streams
* Dev 22 (bulk 0x83, 120.5 Hz, 1242 B, 485 B captured): record structure `01 00 00 00 | len=0x04da (1242) | ...` followed by tagged items (`.. 00 00 00 05 00 00 00 04 00 23 0b b8 ..`). Bytes 70-73 are a **big-endian u32 timestamp in us**, nominal step 8333 us (120 Hz). Versus the IMU clock the offset is -31.64 s and drifts ~ +16 us/s (jitter +-12 ms from USB latency); epoch differs from the IMU uptime. Candidates: gaze/eye-camera meta or controller/hand data (eyetracking capture should settle it, an eyetracking.md already exists in analysis/). Not decoded.
* Dev 18 EP 0x01/0x81: 'X' 25-byte command at 12.5 Hz, e.g. `58 19 51 00 01 32 2f07 2f07 ed0f ed0f 0d00 0d00 1100 1100 2800 3e00`, reply `4c 04 51 00 01 07`. Values pairs are duplicated and slowly vary (looks like display/IPD/lens or pose-time parameter streaming). Undecoded. Low rate `58 04 39 00 48`.
* Dev 18 EP 0x84: text log from the MCU (zero-cost sync reference for the MCU clock).

## 5. Commands used
```
tshark -r captures\20261013-124423-idle-tracking-hub2.pcap -q -z conv,usb
tshark -r <pcap> -T fields -e usb.device_address -e usb.endpoint_address -e usb.transfer_type -e usb.data_len -e usb.urb_type -E separator=";"   # grouped counts
tshark -r <pcap> -Y "usb.device_address==18 && usb.data_len>0 && usb.endpoint_address==0x83" -T fields -e frame.time_relative -e usb.capdata -E separator=,   > imu.csv
tshark ... -Y "usb.device_address==22 && usb.data_len>0" ... > d22.csv
python -I (numpy): per-byte variance table; u32 diff of bytes 4-7; int16 BE words; |b1-b0| interleave test;
  gravity-direction LSQ fit d(g)/dt = g x (M gyro); raw pcap parser for USBPcap iso descriptors (struct '<HQIHBHHBBI', then '<III' start_frame,npkts,err, then 12-byte descs)
```
Scripts live in the session scratchpad (imu_b.py, imu_c.py, imu_d.py, imu_cam.py, imu_d22b.py).
