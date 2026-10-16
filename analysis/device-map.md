# Pimax Dream Air (SLAM edition): USB device map

Source: `20261013-124248-poweron-hub2.pcap` (USBPcap, bus 2, enumeration at t=38.69..40.1 s).
Re-check addresses in later captures with `tools/` scripts: the headset **re-enumerated** at 38.69 s of
the poweron capture, so the address changed from 11 to 18. Captures taken after poweron (idle-tracking,
ipd-brightness, controllers, eyetracking, 125121-shutdown) all use the new addresses (18/20/21).
The 124051-shutdown capture was taken BEFORE the replug and uses the old address **11** for the headset.

| Bus.Addr (after replug) | VID:PID | Device | Notes |
|---|---|---|---|
| 2.18 | 34A4:0044 | Headset composite "Pimax Dream Air" (string idx 2 = UTF-16 "Pimax Dream Air") | USB 2.0 HS, bcdDevice 0x0000, 1 config (wTotalLength 220), 6 interfaces |
| 2.20 | 34A4:0043 | Controller dongle | USB 2.0, bcdDevice 0x0002, 1 HID interface, 1 config (41 bytes) |
| 2.21 | 34A4:0014 | "PimaxSensor" UVC camera (iInterface string idx 4 = "PimaxSensor") | USB **3.2** (bcdUSB 0x0320), class EF/02/01 (IAD), bcdDevice 0x0006, maxpkt0=512 |
| 2.11 (before replug) | 34A4:0044 | same headset, older enumeration (its descriptors not in capture; endpoint pattern identical) | disappears at t=25.78 s |
| 2.15, 2.16, 2.17, 2.19 | n/a | USB hubs (control + 1-5 byte status-change INT IN only) at t=36-39 s: hub chain in front of the headset | not Pimax functions |
| 2.7 , 2.14, others | n/a | non-Pimax (iso / bulk big streams, no descriptors in capture) | ignore |

## 34A4:0044 headset (2.18): interfaces / endpoints
Device descriptor raw: `12 01 00 02 00 00 00 40 a4 34 44 00 00 00 01 02 03 01`

| IF | Class | Endpoints | Role (observed) |
|---|---|---|---|
| 0 | HID (03/00/00) | EP 0x81 IN int 64B, EP 0x01 OUT int 64B, bInterval 1 (1 ms HS) | **Command channel** (host 'X' requests on 0x01, device 'L' replies on 0x81). HID desc len 27 |
| 1 | HID | EP 0x82 IN int 64B | Stream 'S' 0x32 reports (500 pkt/s), only seen active in some captures (see poweron.md) |
| 2 | HID | EP 0x83 IN int 64B | **IMU stream** 'S' 0x31 reports, 500 pkt/s x 2 samples = 1 kHz, starts by itself ~1.1 s after boot |
| 3 | HID | EP 0x84 IN int 64B | **ASCII firmware log** (Zephyr/nRF style `[hh:mm:ss.mmm,uuu] <info> app: ...`, split over 64B reports) |
| 4 | Audio Control (01/01) | none | UAC1 control, feature unit (mute/volume queried at enumeration) |
| 5 | Audio Streaming (01/02) | alt0 none; alt1 EP 0x88 IN iso 192B (UAC format type 1) | microphone: PCM, 2 ch, 16-bit, 48000 Hz (format type I) |

(No output EP exists on IF1-3.  Windows host performed SET_IDLE(0) on IF0-3 and read the HID report descriptors.)

### HID report descriptors (all four identical in structure; NO report IDs)
IF0 (27 bytes): `05 01 09 00 a1 01 15 00 25 ff 19 01 29 08 95 40 75 08 81 02 19 01 29 08 91 02 c0`
= Usage Page Generic Desktop, Usage 0, Collection Application, logical 0..255, **Input 64 x 8-bit**, **Output 64 x 8-bit**, End.
IF1/2/3 (21 bytes): `05 01 09 00 a1 01 15 00 25 ff 19 01 29 08 95 40 75 08 81 02 c0` = **Input 64 bytes only**.
Since there are no Report IDs, the first data byte on the wire is payload byte 0 (the device-defined magic 'X'/'L'/'S').
The device does NOT use SET_REPORT/GET_REPORT control transfers in any capture; all traffic is raw 64-byte interrupt reports
(hidraw read/write on Linux).

## 34A4:0043 dongle (2.20)
Device descriptor raw: `12 01 00 02 00 00 00 40 a4 34 43 00 00 02 01 02 03 01`
IF0 HID: EP 0x81 IN int 64B + EP 0x01 OUT int 64B (interval 1). Report descriptor identical to headset IF0 (27 B, in+out 64B, no report IDs).
Dongle emits ASCII debug text on IN 0x81 at boot (e.g. `[E] ept_init_rtc_timeout`, `etp_ld ld_en:1 ld_s:161`, `etp chip act_s:1`).
Host sent nothing to it in the poweron capture.

## 34A4:0014 PimaxSensor camera (2.21): UVC 1.0-ish
Config total 454 B, 2 interfaces (IAD first IF 0, count 2, class 0E/03).
* VC IF0: INT IN EP 0x83 (32B, interval 11) status. Units: Input Terminal id 1 (camera, type 0x0201?), Output Terminal id 2 (USB streaming) source 4,
  Processing Unit id 3 (source 1), **Extension Unit id 4**, GUID bytes `adccb1c2f6abb8488e3732d4f3a3feec`,
  5 controls, bmControls `06 0e` -> selectors **2, 3, 10, 11, 12**.
  Enumeration probed them (GET_LEN/INFO/MIN/MAX/RES/DEF): sel2 len 6 (def 80x6), sel3 len 4 (def 80x4), sel10 len 1 (def 0x80), sel11 len 16 (min 00..0600 1604..), sel12 len 1 (def 0x80).
  (wIndex=0x0400 = unit 4, IF 0.)
* VS IF1: 1 format, 1 frame:
  * **Uncompressed, GUID 59555932-0000-1000-8000-00AA00389B71 (YUY2), 16 bpp**, frame index 1
  * **1280 x 832**, dwMaxVideoFrameBufferSize 2,129,920, frame intervals **166666 (60 fps)** and **200000 (50 fps)**, default 60 fps
    (min/max bitrate 851,968,000 / 1,022,361,600 bit/s, i.e. a 1280x832 frame is probably the 4 OV7251 640x416 mono sensors tiled; calibration lists trackingA/B + Pi-controllerA/B)
  * alt settings 1..11: iso IN EP 0x81 with SuperSpeed companion descriptors; bandwidth grows with alt number
    (alt 1 wMaxPacketSize 0x3c00 = 1024 B x 4 trans/interval, burst 7; alt 11 = 1024 B, burst 12 : see raw dump in poweron.md).
* Firmware log of the headset (IF3) confirms OV7251 sensors (sensor_id 7750, 4 probed), FSIN PWM 60 Hz, FPGA firmware version 3.
* No UVC probe/commit/streaming in the poweron capture (only a stream start in `20261013-125121-shutdown`: iso EP 0x81, ~60 transfers/s).


# Cold-start addresses (capture `captures2/20261013-130457-cold-start-hub2.pcap`, bus 2)

Everything below is from the capture where the headset was plugged in with no Pimax software running (plug at t=17.5 s, Pimax services at t=52.0 s). Addresses are NEW for this plug: **do not reuse 18/20/21/22**. Pimax devices (VID:PID read from device descriptors in the capture):

| Bus.Addr | VID:PID | Device | Enumerated at t(s) | Interfaces / endpoints | Notes |
|---|---|---|---|---|---|
| 2.26 | 34A4:0044 | Headset "Pimax Dream Air" (mfr "Pimax Corporation", serial `P511X0X000000000001`) | 20.04 | 6 IF, identical to table above: IF0 HID EP 0x01 OUT/0x81 IN (cmd), IF1 EP 0x82 IN (controllers), IF2 EP 0x83 IN (IMU), IF3 EP 0x84 IN (ASCII fw log), IF4 UAC control, IF5 UAC stream (alt1 EP 0x88 iso) | HS, config 220 B. Device descriptor `12 01 00 02 00 00 00 40 a4 34 44 00 00 00 01 02 03 01` |
| 2.28 | 34A4:0043 | **Display/video-bridge MCU** (strings: manufacturer "Pimax Device", product "Pimax", serial `326C327034345...`), NOT a controller dongle | 20.62 | 1 HID IF, EP 0x81 IN int 64 B + EP 0x01 OUT int 64 B (config 41 B) | Host never writes; EP 0x81 is an ASCII log (`<uptime_ms>-><[I]> text`). Owns LT7911UXE DP-to-MIPI bridge, Sony/Novatek panel, fan, RGB LED, ept/activation chip. It receives commands forwarded by the headset (`opt 5`) |
| 2.29 | 34A4:0014 | "PimaxSensor" UVC camera, USB 3.2 (bcdUSB 0x0320) | 21.24 | VC IF0 (INT IN 0x83 32 B), VS IF1 alt 0..11, iso IN EP 0x81 (alt 9 = 32768 B / interval, burst 15, used by Windows), config 454 B | XU probed only (selectors 2,3,10,11,12), never written. Streams 1280x832 YUY2 @ 166666 (60 fps) from t=57.95 |
| 2.30 | 2104:0561 | Tobii-based eye module (strings: serial `XR5DA-P511X0X000000000001`; fw `df9b4b881f-7.32.0`; `/hw/platformtype=XR5EYECHIP`, `/hw/subplatformtype=XR5PIMAXDA`) | **63.25** (powered by headset after command `3c/5 [00 01]` at t=61.65; absent from the bus before) | 1 IF class ff/00/00, EP 0x83 bulk IN 512, EP 0x04 bulk OUT 512, EP 0x05 bulk OUT 512 | TTP protocol, see init-sequence.md section 5 |
| 2.23 | ? (descriptors not captured) | hub (SET_CONFIG 17.69; INT IN 0x81 1-byte status) | 17.69 | | upstream hub the headset cable attaches to (port 4 change at 19.34/19.46) |
| 2.24 | ? | USB3 (SuperSpeed) hub, SET_CONFIG 19.36, status INT IN on EP 0x83 | 19.36 | | camera 2.29 hangs off its port 4 (status bit 0x10 at 20.89, and at 26.65 / 57.87 = camera function-suspend/resume) |
| 2.25 | ? | USB2 (HS) hub companion, SET_CONFIG 19.51 | 19.51 | | port 3 -> headset 2.26 (status 0x08 at 20.05), port 1 -> hub 2.27 |
| 2.27 | ? | USB2 hub inside the headset, SET_CONFIG 20.10 | 20.10 | | port 3 -> bridge 2.28 (0x08 at 20.41-20.57), **port 4 -> eye module 2.30** (0x10 at 63.09 and 63.20) |
| 2.3, 2.9 (and 2.5) | n/a | unrelated PC devices: 2.3 = MSI "MYSTIC LIGHT" HID, 2.9 = USB keyboard | present before | | ignore |
| hub1 pcap | | only the root hub 1.0.0 (control reads) | | | no Pimax device on hub1 |

(Topology is inferred from hub status-change bitmaps: bit n = port n. Medium confidence.)

What each does with NO host software (t=17.5..52.8): headset boots in ~5.5 s, starts the IMU stream on EP 0x83 at 500 pkt/s by itself (first packet t=21.18), parks the 4 OV7251 cameras (log `usr_camera_stop`), sends unsolicited controller status (`0x37`, every 2 s per slot) on EP 0x81, then both ASCII logs go silent. Bridge boot log (`ept_init_rtc_timeout`, `etp_ld ... app_unlock:0`). Camera enumerates but does not stream. Eye module is not on the bus. Panel, audio, fan off. Everything else starts only after the host commands in `init-sequence.md`. Firmware logs: `cold-start-fwlog.txt`.
