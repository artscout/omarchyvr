# Pimax Dream Air: power-on / shutdown USB analysis

Captures: `20261013-124248-poweron-hub2.pcap`, `20261013-124051-shutdown-hub2.pcap`, `20261013-125121-shutdown-hub2.pcap` (USBPcap, bus 2).
Device address map and descriptors: see `device-map.md`. Helper scripts (pure Python 3, no deps, `python -I`): `tools/usbp.py` (USBPcap pcap reader),
`tools/group_interrupt.py`, `tools/dump_log_ep.py`, `tools/control_log.py`, `tools/imu_rate.py`.
Use forward slashes in Windows paths for the scripts (`C:/Users/User/pimax-re/captures/...`).

## 0. Headline findings (read first)

1. **None of the three captures contains a genuine "Pimax Play start" init.** In every capture Pimax Play is already running and talking to the headset:
   * poweron: Play is running against the OLD enumeration (addr 11) for the first 7.9 s, then **exits** (see 3.2). The headset then reboots/re-enumerates (addr 18 at t=38.69 s) and
     for the remaining 7 s of the capture the host sends **zero** OUT reports to it (only enumeration control transfers + reads). The capture ends before Play reconnects.
   * shutdown #1 (124051): Play stays connected the whole 24 s (OUT 0x39/0x51 to the last frame). Only the headset power button was pressed (firmware log `uart_key ... power`).
   * shutdown #2 (125121): 2.4 s long, Play is still connected at the end. No exit seen.
   So the Play-exit signature is available only at t=7.85 s of the poweron capture (addr 11). A real connect/init sequence has to come from a capture started with Play closed.
2. **The whole protocol is raw 64-byte HID interrupt reports (no report IDs, no SET_REPORT/GET_REPORT, no SET_FEATURE).** Commands: host writes on IF0 EP 0x01, replies on EP 0x81.
3. **IMU streams on IF2 EP 0x83 by itself** (no command needed): 500 reports/s, each with 2 samples = 1 kHz IMU, starts ~1.1 s after the headset boots (already running at boot, host only has to read).
4. **IF3 EP 0x84 is the firmware's ASCII debug log** (very informative, gives boot/power/camera/panel/audio event names).

## 1. Enumeration (poweron, t = 38.69 .. 40.1 s)

See `device-map.md` for descriptors. Order: dev 18 (headset) at 38.694, dev 20 (dongle) at 39.272, dev 21 (camera) at 39.883.
Host control traffic to the headset (addr 18) after enumeration, complete list (all other control transfers are standard descriptor reads):

| t (s) | bmReq | bReq | wValue | wIndex | wLen | meaning |
|---|---|---|---|---|---|---|
| 38.6994 | 00 | 09 | 0001 | 0 | 0 | SET_CONFIGURATION 1 |
| 38.7005..38.7054 | 80 | 06 | 0302 | 0409 | 4/32 | GET_DESCRIPTOR string idx 2 (x5, Windows re-reads) = "Pimax Dream Air" |
| 38.7080 | 21 | 0A | 0000 | 0 | 0 | HID SET_IDLE(0) IF0 |
| 38.7082 | 81 | 06 | 2200 | 0 | 91 | GET HID report descriptor IF0 (27 B returned) |
| 38.7114 / 38.7147 / 38.7179 | 21 / 81 | 0A / 06 | ... | 1 / 2 / 3 | | same for IF1, IF2, IF3 (21 B each) |
| 38.7216..38.7221 | A1 | 81 | 0100,0101,0102 | 0204 | 1 | UAC GET_CUR on feature unit 2 (mute/vol), all returned 00 |
| 38.7223 | 00 | 0B | 0000 | 0005 | 0 | SET_INTERFACE IF5 alt 0 (audio streaming idle) |

Nothing else is sent to IF0..IF3 afterwards in this capture. Dongle (addr 20): SET_CONFIG, SET_IDLE, report descriptor only.
Camera (addr 21): SET_CONFIG at 39.886, string idx 4 ("PimaxSensor"), SET_INTERFACE IF1 alt 0, then XU probing: GET_LEN/GET_INFO/GET_MIN/GET_MAX/GET_RES/GET_DEF
on Extension Unit (wIndex 0x0400) selectors 2,3,10,11,12 (see device-map.md). No SET_CUR, no PROBE/COMMIT, no iso video in the poweron capture.

Reproduce:
```
tshark -r poweron-hub2.pcap -Y "usb.device_address==18 && usb.transfer_type==0x02" -T fields -e frame.number -e frame.time_relative -e usb.control_stage -e usb.bmRequestType -e usb.setup.bRequest -e usb.setup.wValue -e usb.setup.wIndex -e usb.setup.wLength -e usb.capdata
tshark -r poweron-hub2.pcap -Y "usb.bDescriptorType==1 && usb.idVendor" -T fields -e frame.number -e usb.device_address -e usb.idVendor -e usb.idProduct   # addr -> VID:PID
python -I tools/control_log.py <pcap> 18,20,21        # setup + data stage paired, human readable
```
(`usb.bmRequestType` is not a valid tshark field name in this build; use the python helper for paired setup/data output. The data stage is a separate frame, `usb.capdata`.)

## 2. Message format on IF0 (command channel)

All 64 bytes, zero padded. Host -> device on EP 0x01 (`usb.endpoint_address==0x01`), device -> host on EP 0x81.

```
request  : 58 LL CMD OPT DATA...        'X'; LL = 4 for short commands, 0x19 for CMD 0x51
response : 4c 04 CMD OPT 01 07          'L'; echo of CMD/OPT, 01 = ok(?), 07 = unknown (constant in all captures)
```
(Log lines of the firmware: `cmd 41 opt 0 datalen 1`, i.e. the firmware calls byte 2 "cmd" (decimal 41 = 0x29) and byte 3 "opt".) Fields after OPT in short commands are 16-bit LE values.

Commands seen (all captures, both devices addresses):

| CMD (hex/dec) | OPT | Payload (hex) | Direction/reply | Frequency | Meaning (confidence) |
|---|---|---|---|---|---|
| 0x39 / 57 | 00 | `4800` (=72) normally, `0000` at exit | no reply | every ~0.3-0.4 s (n=39 in 7.9 s, 115 in 24 s) | **PC keepalive/heartbeat with current video rate in Hz** (72 -> 0 when video stops). Medium. Firmware `pc_heartbeat_timeout` fires 5.0 s after the last one. High for "heartbeat"; the 72 Hz meaning is a guess (matches `required_video_rate` log text) |
| 0x51 / 81 | 00 | `01 32 | u16 a | u16 a | u16 b | u16 b | 0d00 0d00 | u16 c | u16 c | 2800 | 3e00` (25 data bytes) | reply `4c 04 51 00 01 07` every time | every 80 ms (12.5 Hz) | periodic status/telemetry push, 4 duplicated u16 pairs (a: 0x0602..0x0cA6 slow steps, b: 0x0e00-0x1000 noisy 3600-4100 = sensor-like, c: 13..116 smooth ramp). 0x32=50 and tail 0x28(40), 0x3e(62) constant. Semantics unknown (low). Each value appears twice (left/right eye?) |
| 0x29 / 41 | 00 | `00` or `01` | reply `4c 04 29 00 01 07` | 3x in shutdown#1: t=1.548 (0), 15.651 (1), 17.814 (0) | **panel on/off request** (`eSpiCmdPanelOnReq data 1/0`, `request_panel_on`). High (firmware log in the same capture) |
| 0x1a / 26 | 05 | `04` | reply `4c 04 1a 05 01 07` | once, poweron t=7.854 | PC notify / "heartbeat notify required_video_rate" (log `eSpiCmdPcHeartbeat notify required_video_rate 0` right after). Sent just before Play stops; part of the exit signature. Medium |
| 0x26 / 38 (13 B) , 0x27 / 39 (11 B), 0x2a / 42 (4 B), 0x04, 0x16, 0x50, 0x0a, 0x03 | | | | other captures (ipd-brightness, controllers) | out of scope; **0x26/0x27/0x2a x5 in ipd-brightness are likely IPD/brightness**. Listed so other analysts can grep |

No ASCII serial/firmware/JSON strings appear in host->device payloads. The only ASCII is the device's own debug log (section 5).

## 3. Sequences

### 3.1 Poweron capture, addr 11 (before headset reboot), t = 0 .. 7.9 s
Steady state of a running Play session (no init visible):
```
t=0.006       OUT  58 04 39 00 48 00 ...            heartbeat (72)       every ~0.3-0.4 s
t=0.057       OUT  58 19 51 00 01 32 98 08 98 08 bb 0f bb 0f 0d 00 0d 00 17 00 17 00 28 00 3e 00  -> IN 4c 04 51 00 01 07   every 80 ms (99x)
```
### 3.2 Poweron: Play exit signature (t = 7.854 .. 7.883)
```
t=7.854  OUT 58 04 1a 05 04 00 ...        cmd 26/5 data 04       -> IN (t=7.904) 4c 04 1a 05 01 07
t=7.855  OUT 58 04 39 00 00 00 ...        heartbeat with video rate 0
t=7.877  OUT 58 04 39 00 00 00 ...        again
t=7.881  last 0x51 push (+ reply 7.883); then silence
```
Firmware reaction (IF3 log, wall clock 00:58:xx, t=log-00:58:33.32):
* 7.86: `cmd 26 opt 5 datalen 1` / `eSpiCmdPcHeartbeat notify required_video_rate 0`
* 7.9..8.15: audio shutdown: `audio_ctrl_cmd:2`, Awinic aw883xx amplifiers stopped (`aw883xx_device_stop`), `codec_enable switch:1 enable:0`, codec i2c writes 0x1800C006=0, 0x1800C036=0xAFAF (mute)
* 10.6: `set_indis_from_stm32 release`, `request_panel_on 0` (panel off: triggered by firmware itself 2.7 s after exit)
* 12.9: `pc_heartbeat_timeout!!!!` (5.0 s after last heartbeat), `device manager old:1 pc_alive:0`
* 25.78: device drops off the bus (all EP IRPs cancelled) -> headset power-cycled (reboot/power-off); re-enumerates as addr 18 at 38.69 s.
* New boot log (IF3 at t=39.5, uptime 1.7 s): FPGA fw load (`ga_fw_update_thread`, `fpga firmware version:3`), 4x OV7251 power-up/init/`camera_probe ok id 1..3` (sensor_id 7750), `start_fsin_pwm b_60hz 1`, `usr_camera_stop` at uptime 5.48 (cameras idle until host UVC opens them).

### 3.3 Shutdown #1 (124051, addr 11, 24.4 s, Play NOT closed)
Heartbeat + 0x51 pushes continue to the last frame (t=24.39), IMU streams to the end. Three PanelOnReq commands (0x29): off at 1.548, on at 15.651, off at 17.814.
Matching firmware log: `uart_key power:[1]` (physical button) at 1.54/15.6/17.8 -> `eSpiCmdPanelOnReq`, audio amp stop/start (`aw883xx_start`, codec register sequence: 0x1800C074=0x0C, C092=0x8000, C094=0x8080, C034=0x4F4F, C09A=0xC840, C038=0xCFCF, C054=5, C006=0, C036=0x2F2F on; C006=0, C036=0xAFAF on stop). So Play translates headset power-button presses into `0x29` panel commands (or the button handler relays through Play).
### 3.4 Shutdown #2 (125121, addr 18, 2.4 s)
Only steady state (0x39 x11, 0x51 x30). EP 0x82 (IF1) streams 500 reports/s and iso video is flowing from the camera (EP 0x81 of 2.21, ~60 xfers/s) because the previous eyetracking/tracking session was still active. No exit commands.

**Comparison with init:** no init could be captured, but exit = [0x1a/5 data 04] + [0x39 data 0 x2] + stop sending; panel-off and audio-codec shutdown are firmware reactions, not host commands. Power-on is therefore expected to be the reverse: start sending 0x39 (video rate) and 0x51 every 80 ms, then 0x29 data 1 (panel on) - UNVERIFIED, needs a capture beginning with Play closed.

## 4. Streaming IN reports

### 4.1 IMU: IF2, EP 0x83, 64 B, **500 reports/s (2 ms spacing, 1 kHz samples)**
Measured: 12,893 reports in 25.78 s (addr 11) and 3,041 in 6.08 s (addr 18) = 500.2 /s; timestamp delta 1999/2000 us.
No command required: first report at 39.826 s on addr 18 (headset boot + ~1.1 s).
```
off  size
0    1   0x53 'S'
1    1   0x31 report type (IMU)
2    1   0x08  (constant; probably payload size/sample count code)
3    1   0x00
4    4   u32 LE timestamp, microseconds since headset boot (verified: 58 min uptime = 0xD1691A96 us on addr 11)
8    4   u32 constant: 0x2b (addr 11) / 0 (addr 18) - unknown (boot counter/epoch?)
12   16  sample 0:  u8 tag(0x0c/0x0d) | i16 x6 @13,15,17,19,21,23 | i8 @25 | u16 @26 (~1000, 999/1000)
28   16  sample 1:  same layout (the two samples are 0.5 ms apart; sample timestamps inferred = packet ts, packet ts + 1000 us (assumption))
44   20  45..47 slow-changing status bytes (0x42/0x43, 0x18, 0x88/0x80/0xca...), rest 0
```
* i16 #0..#2 = **accelerometer, 2048 LSB/g (+-16 g)**: |a| = 2089 (still, addr 11) and 2051 (moving, addr 18 controllers capture) -> confidence high. Axis order/sign unverified.
* i16 #3..#5 = gyroscope (noise sd 40-45 LSB at rest, bias ~120/13/4): scale unknown (131 LSB/dps would give +-250 dps; check against `device_calibration_*.xml` kg/ombg or a slow rotation capture). Medium-low.
* `u16 ~1000` at byte 26/42 and `i8 @25` (small signed, ~temperature or sample index): unknown.
Reproduce:
```
tshark -r poweron-hub2.pcap -Y "usb.device_address==18 && usb.endpoint_address==0x83 && usb.capdata" -T fields -e frame.time_relative -e usb.capdata
python -I tools/imu_rate.py <pcap> 18
```
### 4.2 IF1 EP 0x82 reports (500/s, seen only in shutdown#2): `53 32 08 02|03 43 80 80 00 <u32 us ts> ...`
Alternating subtype 02 / 03 each 2 ms (=250 Hz each), contains pairs of fields (`01 ac 03 9d`, `ff fc ...`) that look like a second sensor group (possibly controller-tracking/eye or camera-sync IMU). Not active in poweron (the host only has the read pending). Low confidence; belongs to the eyetracking analyst.
### 4.3 Log: IF3 EP 0x84, ASCII, 64 B chunks (zero padded, lines straddle reports), ~100 reports/s during bursts.
### 4.4 Dongle (addr 20) IN 0x81: ASCII text at boot only.
### 4.5 Camera: no iso video in poweron (nobody opened the stream). (125121-shutdown has it: 60 transfers/s on EP 0x81 of 2.21, ~1.6-2.0 MB per URB.)

## 5. Useful strings (device-originated, from IF3 log)
`eSpiCmdPcHeartbeat`, `eSpiCmdPanelOnReq`, `request_panel_on`, `pc_heartbeat_timeout`, `device manager ... pc_alive`, `set_indis_from_stm32`, `uart_key up/down/power`,
`ov7251_*` camera init (4 sensors, sensor_id 7750), `start_fsin_pwm b_60hz`, `fpga firmware verison:3`, Awinic aw883xx audio amp + codec i2c writes. No serial/version string appeared in the logs or payloads of these captures; GET_DESCRIPTOR serial (string idx 3) was never requested.
Dump the log: `python -I tools/dump_log_ep.py <pcap> <addr> 84`.

## 6. Open questions
1. Real connect/init sequence: need a capture starting with Play closed and the headset already enumerated (then start Play) - best on the same hub2.
2. Semantics of cmd 0x51 fields (a/b/c) and the constant 0x32; cmd 0x39 value 0x48 (72 Hz?) vs display mode; cmd 0x29 data values.
3. What enables IF1 EP 0x82 stream and the camera 4-sensor tiling (1280x832 YUY2, 60/50 fps) - needs UVC PROBE/COMMIT capture from Play start.
4. Gyro scale/axis conventions; meaning of the bytes at 8..11, 25, 26..27, 45..47 in the IMU report.
5. Why the headset power-cycled at 25.8 s in the poweron capture (user action or Play-exit side effect?).
