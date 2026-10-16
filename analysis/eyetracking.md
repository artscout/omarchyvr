# Eye tracking analysis (capture 20261013-124906-eyetracking-hub2.pcap, 130.9 s)

Bus 2 device addresses are per-capture (Windows re-enumerates); the Tobii-like device was **addr 14** in poweron/shutdown and **addr 22** in idle/eyetracking. No VID:PID seen (no enumeration); identify by traffic.
Tools: tshark 4.6, python -I. Per-device TSV: `tshark -r X -Y "usb.device_address==18||..." -T fields -e frame.number -e frame.time_relative -e usb.device_address -e usb.endpoint_address -e usb.transfer_type -e usb.irp_info.direction -e usb.capdata`
Summary table of all captures: `tshark -r F -Y "usb.irp_info.direction==1" -T fields -e usb.device_address -e usb.endpoint_address -e usb.transfer_type -e usb.data_len | awk ...` (completions only).
hub1 files: only root-hub (1.0.0) control descriptor reads, unrelated.

## 1. Endpoint inventory (completions, eye vs idle)
| dev | ep | type | idle (56 s) | eye (131 s) | what |
|---|---|---|---|---|---|
| 18 (headset 34A4:0044) | 0x01 OUT / 0x81 IN | intr 64B | 974/708 | 2252/1637 | HID cmd/poll: OUT `58 19 51 00 01 32 ...` polled ~12.5 Hz, reply `4c 04 51 00 ...`; `58 04 39 00 48..` extra. Same as idle (values in the 0x58 19 packet at +8..+13 are u16 counters/brightness-like ramps, same as idle). |
| 18 | 0x83 | intr 64B 500 Hz | yes | yes | IMU: `53 31 08 00` + u32 us timestamp + int16 triplets (3 samples/pkt) |
| 18 | **0x82** | intr 64B 500 Hz | **absent** | **65412 pkts** | NEW vs idle, but ALSO present in controllers capture (14723 pkts) and shutdown #2. Header `53 32 08 02/03` alternating (250 Hz each type), u32 timestamp, int16 groups (`0x0007c1`...). Looks IMU/pose-like (controller/hand related), NOT eye-specific: its content varies with motion, rate is constant, and it appears in the controllers capture without eye tracking. |
| 18 | 0x84 | intr | 4 | 8 | text log, ASCII ("[00:06:28.188] <info> app: <wakeup_dev_check_t...") - firmware debug log |
| 21 (cameras 34A4:0014) | 0x81 | **iso** | 3540 | 8178 | ~62 URB/s, same size (~1.7 MB URBs) as idle. Only ONE iso IN endpoint is ever seen -> a single UVC stream (the 4 SLAM cams), no second stream/format appears during eye tracking. |
| 22 (Tobii-like) | 0x05 OUT / 0x83 IN | **bulk** | stream: 6823 x 1242 B (120 Hz) | **only 47 cmd/47 resp (32 B replies)** | see below |
| 22 | ctl | | | 8x vendor `bmReqType 0xC1 bReq 70 wValue 1 wLen 8` at t=7.75 s | device init reads |

## 2. How eye data reaches the PC: ready-made gaze data over a separate bulk device (TTP protocol), NOT raw images
A bulk device with EP 0x05 OUT / 0x83 IN (addr 14 or 22) speaks a Tobii Transport Protocol (TTP)-like framing:
```
wrapper: u32 LE flags (0=OUT, 1=IN) | u32 LE total length
TTP hdr (big endian): magic u32 (0x51 request, 0x52 response, 0x53 notification) | seq u32 | u32 0 | (resp: 1/0) | opcode u32 | payload size u32
TLV payload: tag u8 | len u32 BE | value.  tags: 5=u32 id, 2=int32 counter, 3=fixed S15.16 (int32/65536), 6=int64 timestamp(us)
```
Notification stream (magic 0x53, opcode 0x4ba, stream id 0x0505), 1242 bytes, **120.5 Hz**, seen in idle/ipd/controllers/poweron/shutdown. Snaplen 512 truncates to the first 458 bytes (≈ first 48 TLVs), enough to decode:
- item 3 (tag 6): int64 timestamp us (monotonic, +8.3 ms per pkt)
- items 7,8,9 (S15.16): eye/gaze origin in mm, e.g. (31.09, -6.19, -25.74)  [0,0,0 when invalid]
- items 16,17,18: **unit gaze direction** e.g. (0.139, 0.1645, 0.9765) (|v|=1.0)  [0 when invalid]
- item 24: pupil diameter mm (3.63, 3.87); -1.0 = invalid
- items 36,37: 0.49 / 0.63 normalized 2D (likely gaze point / openness); -1.0 invalid
- the structure repeats per record (tag-5 ids 0x20bb9 / 0x31f42 mark sub-records, tag-2 numbering them 1..N): rest of the 1242 B is cut off by snaplen (probably the second eye, validity flags, more fields). Fields are **fixed-point S15.16 big-endian, not 32-bit float**; test of LE floats in [-1,1] fails (values like 1.4e-42), BE-fixed works.
Verify: python parse over `usb.device_address==22 && ep 0x83` in 20261013-124423-idle-tracking-hub2.pcap, items extracted at record offsets above.
=> The tracker returns computed gaze (origin + direction + pupil), not eye images. No eye-camera images on USB at all.

## 3. Host->device commands (eye capture, OUT ep 0x05 to Tobii dev), all TTP request 0x51
- 7.77 s: op 0x4c4 payload tags `5:0x0509`, `...`  (likely unsubscribe/stop of stream 0x0509 / session reset) - the 120 Hz 0x0505 stream is gone after this and never resumes in the capture.
- Repeating calibration cycle (every ~37 s, 5x): op 0x3f2 (no payload, "get status"/start calibration), op 0x407 x3-4 with payload `[00000008 <S15.16 x> 00000000 y? ]` = **calibration target coordinates**: pairs from {0.0, 8.375 (0x86000), -0.375 (0xfff7a000), 0x1f400=1.9}, then op 0x42e + 0x3fc (finish/compute calibration), then 0x3f2 again. Example OUT hex: `...0407 0000 0000 0000 0032 | 04 00000008 00086000 00000000 | 04 00000008 fff7a000 00000000 | 04 00000008 001f4000 00000000 | 02 00000004 00000003`.
- 129.26 s (end): op 0x4ce payload `tag0b: 0x0505` = **subscribe stream 0x0505** (same op seen at poweron t=10.59 s, after which the 0x4ba 1242 B stream follows ~15 s later). So 0x4ce(0x0505) starts the gaze stream; 0x4c4(0x0509) is the counterpart stop/other stream.
- Exact semantics of op codes 0x3f2/0x407/0x42e/0x3fc are inferred from timing/payload, not verified; IR LED control was NOT seen as a separate command (illumination is internal to the Tobii module, firmware-controlled).
- On the headset HID (18 ep1) no eye-related command appeared; the 0x58 0x19 poll is identical to idle.
- Calibration: the points are sent to the module (host-driven target list) and the module does the fit; Windows runtime kept the gaze stream OFF during calibration (stream unsubscribed at start), so we see no calibration gaze samples in this capture.

## 4. 34A4:0014 camera device
Device 21 shows one iso IN endpoint (0x81) in all captures, one stream, constant rate/size => 4 SLAM cameras multiplexed in one UVC stream. No second endpoint/interface for eye cameras. Config descriptors not captured (no enumeration) so the interface list is unverified; re-capture on replug to confirm.

## 5. Feasibility for Linux
Good. Gaze arrives already computed (S15.16 origin/direction/pupil at 120 Hz) from a simple bulk device with a TTP-like protocol. Needed: (a) full 1242-byte frames (recapture with snaplen 65535), (b) the full init sequence from poweron (enumeration + 0x4ce subscribe + vendor reads 0xC1/70), (c) the calibration command semantics (0x3f2/0x407/0x42e/0x3fc) if device-side calibration is needed; per-user calibration is stored in the module / driven by host, still reproducible. No closed Tobii algorithm needed for raw output; Monado XRT_INPUT_GENERIC_EYE_GAZE_POSE can be fed directly.
