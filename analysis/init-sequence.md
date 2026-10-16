# Pimax Dream Air (SLAM): cold-start INIT SEQUENCE (Windows host, Pimax Play)

Source: `captures2/20261013-130457-cold-start-hub2.pcap` (USBPcap bus 2, snaplen 512, 88.2 s, 98,780 frames).
Companion files: `cold-start-fwlog.txt` (both firmware logs, timestamped), `headset-calibration.xml` (decoded from the headset, section 6), `device-map.md` (section "Cold-start addresses").
Time base: `t` = seconds since pcap start; wall = 13:04:58.029 + t (+08:00). Marks: plug-in about t=17.5 (13:05:15.5), "idle without Pimax" mark t=51.71, services t=52.03 (13:05:50.056), Play t=52.04, first command to the headset t=52.799 (13:05:50.83), stop t=88.18.
Confidence: H = proven by a firmware log line or reply, M = consistent with logs/timing, L = guess.
Helper scripts (scratch, re-creatable): all analysis used `tools/usbp.py` (`usbp.read()`), plus tshark for VID/PID.

## 0. Headlines

1. **After plug-in the headset runs by itself until the host sends the first `X` command**: it boots (~5.5 s), probes the 4 OV7251 sensors, then parks. IMU (EP 0x83, 500 pkt/s) is streaming from t=21.18 (uptime ~3.6 s) with no host command. Nothing else streams. Firmware log (EP 0x84) and bridge log go silent from t=23.1 until t=52.8. Panel off, cameras off, fan idle.
2. **Everything that turns things on is sent over the single HID command channel (IF0, EP 0x01 OUT / 0x81 IN)**. Windows sends ZERO vendor control transfers to the headset (34A4:0044) and ZERO to the bridge (34A4:0043). Not a single UVC extension-unit SET_CUR was sent to the camera. The only vendor control traffic is on the Tobii eye module (2104:0561).
3. **The "dongle" 34A4:0043 is NOT a controller dongle. It is the display/video-bridge MCU** (Lontium LT7911UXE DisplayPort-to-MIPI bridge, Novatek NT panel driver, Sony panel power, fan, RGB LED, audio codec amp control, "ept" activation/crypto chip). The host never writes to it directly: commands with `opt = 5` on the headset channel are forwarded to it (its log shows the same `cmd N opt 5` lines ~8 ms later). Its EP 0x81 is an ASCII log only.
4. **Controllers need no pairing in a normal start**: pairing is persisted in the headset radio. The headset itself announces them (unsolicited `0x37` notifications every 2 s per slot, from t=20.66, i.e. before any host command). The 0x82 report stream starts only after the host sends the 4-command "attach" burst per slot (section 4, phase F).
5. **Video/panel**: host tells the headset "PC alive" (`0x1a opt5 [01 00 00]`) -> the bridge powers up DP receiver + panel supply, the DP link appears to the GPU about 1 s later. The host then announces `required fps = 72` (`0x1a opt5 [02 48 ..]`) and (Windows) starts scanning out a 72 Hz mode. The firmware counts stable DP frame-rate samples (every 0.5 s) and, after 11 stable hits (~5.5 s), enables the MIPI TX and the panel, then backlight (brightness is NOT sent in init: it keeps its stored value 0x5e = 94 % = backlight 239) and audio amps. Panel ON appeared 14.4 s after the first command, 9 s after `required fps`.
6. **New find: the headset serves its factory camera/IMU calibration XML** (zlib, 3410 B) via `0x0a` ReadDocument (section 6). 4 cameras (trackingA/B, Pi-controllerA/B, fisheye, 640x480 each) with extrinsics and IMU bias/scale.
7. **Open risks for Linux**: (a) an "activation" handshake (`0x55`/`0x5b`/`0x56`, signed blobs, serial-bound, SHA256+RSA(?) checked by firmware: `ept_vef_sig`) is performed by Play at t=58 and t=65.9 and again at t=69.0; it is NOT proven to gate the panel (see 7.1); (b) the eye module needs a challenge/response (TTP ops 0x76c/0x777) that cannot be replayed verbatim (see 5.3).

## 1. Opcode table (command channel, 64-byte HID reports, no report ID)

Framing (all captures): host `58 LL CMD OPT DATA..` zero padded to 64 B, `LL = len(DATA)+3` (the byte after DATA is always 0, a reserved/checksum slot that the firmware does not check; firmware logs `datalen = LL-3`). Device `4c LL CMD OPT DATA.. CK`, `LL = len(DATA)+2` incl. CK; **CK = trailing byte, algorithm unknown** (not sum/xor/CRC8 over any span, tested all 256 polys x 256 inits x 4 spans on 60 replies). Plain acks are `4c 04 CMD OPT 01 07` (01 = ok, 07 constant). Replies arrive 1-20 ms after the request on EP 0x81 (one in flight is safe; Windows pipelines 2-3 requests).
`OPT` = target subsystem: 0 headset MCU; 2 / 3 = controller slot left / right (radio); 5 = display bridge MCU (34A4:0043, forwarded); 4, 6, 9 = other sub-controllers answering only `0x03` here (FPGA / sensor boards?, L).
Names in the "fw name" column are strings printed by the firmware (`eSpiCmd...`).

| CMD hex (dec) | OPT | Request DATA (hex) | Reply DATA (hex, w/o CK) | fw log name | Meaning | Conf |
|---|---|---|---|---|---|---|
| 0x03 (3) | 0 | none (`58 03 03 00`) | `00 00 21 01` | eSpiCmdSoftVer | read software version of target (opt 0 = HMD MCU). Version encoding unknown | H name / L value |
| 0x03 | 2 / 3 | none | `00 08 2d` / `00 08 2e` | eSpiCmdSoftVer | controller fw (left / right); same bytes appear inside 0x37 | H |
| 0x03 | 4 | none | `01 01` | eSpiCmdSoftVer | sub-device 4 version | L |
| 0x03 | 5 | none | `00 00 20 01` | eSpiCmdSoftVer | bridge MCU (34A4:0043) version (its log: `eSpiCmdSoftVer`) | H |
| 0x03 | 6 | none | `ff ff ff 0f` (t=52.93, early boot), later `01 02 06` | eSpiCmdSoftVer | sub-device 6 (seen on both logs); the value changes once the video path is up (`ff ff ff 0f` = not ready?) | L |
| 0x03 | 9 | none | `00 00 09` | eSpiCmdSoftVer | sub-device 9 | L |
| 0x04 (4) | 0 | `slot` (2/3) | ack | eSpiCmdStartPair | start pairing (NOT used in a normal start; see controllers-settings.md) | H |
| 0x09 (9) | 0 | none | `00 03 00` | eSpiCmdHwVer | hardware version 3.0(?) | M |
| 0x0a (10) | 0 | `01 AL AH 10` | `01 AL AH 10 <16 data bytes>` | eSpiCmdReadDocument `_data_type 1, _addr N, _data_len 16` | read 16 B of document type 1 at byte offset N (u16 LE, steps of 0x10). Type 1 = zlib calibration XML (section 6). 84 reads, 2.4 ms apart | H |
| 0x0a | 5 | `02 AL AH 20` | `02 AL AH 20 <32 B>` | eSpiCmdReadDocument + `ept_r_ept base_sn:` | read "ept" record (type 2, 291 B at addr 0..0x120): starts with ASCII serial `P511X0X000000000001\0`, then activation record (signed). 10 reads | H |
| 0x0a | 2 / 3 | `00 00 00 10` | `00 00 00 10 <16 B>` | (controller) | controller radio address/ID: left `06 04 b6 49 ca`, right `06 19 97 9c 11 2a` | M |
| 0x16 (22) | 0 / 2 / 3 | `1e` | `13 <19 ASCII>` (+CK) | xr2_cmd_readSNmain 30 | read serial number (max len 0x1e). HMD `P511X0X000000000001`; left `H30100X000000000001`; right `H30101X000000000001` | H |
| 0x19 (25) | 0 | none | `ff ff 30 ea ae 0f 00 00 a0` | eSpiCmdDeviceCheckResult | self-test result flags (meaning unknown) | L |
| 0x1a (26) | 5 | `01 00 00` | ack | cmd 26 opt 5 datalen 3; HMD: `eSpiCmdPcHeartbeat notify pc state is switched to alive`; bridge: `eSpiCmdVideoCtrl cmd 1 / video set work mode 0` | **"PC is alive / start video session"**; wakes cameras (usr_camera_resume + 4x OV7251 init + `start_fsin_pwm b_60hz 1`), panel supply (`sony_panel_power_on`), DP bridge (`lt7911uxe_power_on`), fan, display state -> WAIT_VIDEO_STABLE | H |
| 0x1a | 5 | `02 48 00 00 00 00 00 00 00 00` (datalen 10) | ack | `eSpiCmdVideoCtrl cmd 2 / open video prepare / requiredFps 72 / keep upscale mode: 0`; HMD: `notify required_video_rate 72` | tell the firmware the refresh rate the PC will output. byte1 = rate in Hz (0x48 = 72); byte2 = upscale mode(?) | H (rate), L (rest) |
| 0x1a | 5 | `05 00 00 00 00 00 00 00 00 00` | ack | `eSpiCmdVideoCtrl cmd 5 / use GPU cfg: Nvidia` | GPU-vendor config; byte1 = 0 means Nvidia (other values unseen: AMD/Intel?) | M |
| 0x1a | 5 | `04` (from poweron.md) | ack | `required_video_rate 0` | stop video / PC leaving | M |
| 0x1b (27) | 5 | `01` | `01 0a 00 00 00` | eSpiGetVideoCtrlInfo cmd 1 ... `return 1 10 0 0 0` | read video-control state (0x0a = 10) | M |
| 0x26 (38) / 0x27 (39) | 0 / 5 | 0x27: 8-byte NUL-padded ASCII key. 0x26: key + `LEN VALUE..` | 0x27: key + `LEN VALUE..` | cmd 39 / cmd 38 | **generic key/value store get/set**. Keys seen in this capture (all GET): `lens_rd` opt5 -> `0f 0f 00 00 00 05 00 99 1d 99 1d f6 ff f6 ff 01` (lens parameters, 16-bit LE pairs 0x1d99/0xfff6, meaning unknown); `BildVer` -> `03 01 00 03` (build version 1.0.3?); `curfms` -> `01 00`; `pbrg` -> `01 5e` (panel brightness 94 %). SET `pbrg` documented in controllers-settings.md | H (protocol) |
| 0x29 (41) | 0 | `01` | ack | (`request_panel_on`/PanelOnReq in other captures) | "panel on request" from PC. Sent once at t=58.047; its log line was lost ("Logs dropped"). Panel actually turned on only at t=67.2 after video-stable, so this is a permission/request, not a direct switch | M |
| 0x2c (44) | 0 | `00` | `1c 54` | eSpiCmdGetVisorStatus | visor (wear / proximity?) status | L |
| 0x37 (55) | 2 / 3 | **device -> host only, unsolicited, every 2.0 s per slot** | `48 <u32 LE> <battery%> 00 02 01 00 08 2d|2e` | none | controller status heartbeat: battery 0x49 = 73 % (left, slot 2), 0x63 = 99 % (right, slot 3), fw bytes `08 2d`/`08 2e`; the u32 (0x0291b6e8, decreasing ~4001 per 2 s) is unknown (timer/RSSI?). Present before any host command | M |
| 0x39 (57) | 0 | `RR 00` (u16 LE: required video rate) | none | (PC heartbeat; `pc_heartbeat_timeout` after 5.0 s silence) | **host keepalive every ~213 ms**. `00 00` from t=52.87 until the video was announced, `48 00` (72) from t=58.41 on | H |
| 0x3a (58) | 5 | `00` | `00 12 7e` | eSpiCmdVisionSts: `displayid:0x2 lensid:0x0 boardid:0x3 lighthouse:0x0 sku:0x1` | hardware variant ids (packed in the 3 reply bytes; packing unknown) | H name / L packing |
| 0x3c (60) | 5 | `00 01` | `00` | eSpiCmdeyetkctrl: [0] | **eye-tracker module control**: sent at t=61.645; the Tobii module (2104:0561) appears on the bus 1.6 s later (t=63.25, power switched on by the headset). Value 01 = enable (L: confirm 00 = disable) | M |
| 0x50 (80) | 0 / 2 / 3 | none | HMD `e8 03 90 d0 87 10 10`; ctrl `f4 01 a0 a0 8f 10 10` | eSpiCmdReadImuConfig | IMU config: 0x03e8 = 1000 Hz (headset), 0x01f4 = 500 Hz (controllers); other bytes unknown | H (rate) |
| 0x51 (81) | 0 | `01 32 AA AA BB BB 0d 00 0d 00 0d 00 0d 00 28 00 3e 00` (u16 LE; AA,BB ramps; datalen 22) | ack `01 07` | none | **periodic push every 80 ms (12.5 Hz)** from t=59.851 to the end. Same format as controllers-settings.md 1.2. Sent right when the firmware re-inits the 4 cameras (t=59.9: `update_fsin_pwm current_hz 50`; 0x32 = 50 matches). Probably camera sync/exposure timing (L) | M |
| 0x55 (85) | 5 | `KK II NN <NN bytes>` (datalen 54, NN=0x33=51, last chunk shorter): KK = 02 activation record, 01 ept record; II = chunk index | `KK II 01` | eSpiCmdencryptw -> `ept_hid encpt_actv size:[293]` / `encpt_ept size:[401]` | upload of a signed blob in 51-byte chunks: KK=02: 6 chunks = 293 B (serial `P511X0X000000000001` + signed data, `ept_act ... act_s:1 tm:1791867962` = current Unix time); KK=01: 8 chunks = 401 B (`ept_eqt ... exp_len:100`) | H (structure) |
| 0x56 (86) | 5 | `IDX` (datalen 1) or `0b EE 00 LL` (datalen 4: read ept expansion reg EE, LL bytes) | IDX: 1-5 value bytes; `0b EE 00 LL <LL bytes>` | eSpiCmdencryptr -> `etp_sts exp_fw / act_s / ept_s / app_s / force`, `ept_r_ept expand:E LL 268435768` | read ept/activation status registers. IDX 0a -> `02` (exp_fw 2), 03 -> `02`, 07 -> `01` (act/app), 06 -> `01` (force), 05 -> `ff ff ff ff`, 01 -> `01`, 00 -> `01`. Polled every ~0.5 s around the handshake | M |
| 0x5b (91) | 5 | `02 02` or `01 64` | ack `01` | eSpiCmdencryptstsw: `ept_w_s write cmd:2 val:2` / `cmd:1 len:100 max:100` | write ept status: `02 02` -> `app_unlock:1, app_s:1` (t=58.046); `01 64` = announce a 100-byte expectation before the 0x55 KK=01 upload | M |

Replies to periodic/unknown commands from the device that Windows ignores: none observed.

## 2. What the headset does alone (no host software), t=17.5 .. 52.8

| t (s) | Event |
|---|---|
| 17.52-19.51 | hub chain enumerates (devs 23, 24, 25, topology in device-map.md) |
| 20.04-21.24 | headset (26), hub (27), bridge 34A4:0043 (28), camera 34A4:0014 (29) enumerate. Windows: SET_CONFIG, HID SET_IDLE(0) x4, report descriptors, UAC feature-unit GET_CUR x3, SET_INTERFACE IF5 alt0; camera: XU selector GET_LEN/INFO/MIN/MAX/RES/DEF probing, SET_INTERFACE IF1 alt0, USB3 function-suspend SET_FEATURE toggles |
| 20.06 | host starts polling all four headset INT-IN endpoints (0x81/0x82/0x83/0x84) and bridge EP 0x81 |
| 20.66 .. 66.9 | unsolicited `0x37` controller status on 0x81, alternating slot 2 / slot 3 every 2.0 s |
| 20.88-23.14 | buffered boot log: FPGA 0.1.1, `fpga firmware version:3`, 4x OV7251 `camera_probe ok id 0..3 sensor_id 7750`, `start_fsin_pwm b_60hz 1`, `fpga_ul_check_thread uploaded`, then `usr_camera_stop ... power off` at uptime 5.49 (sensors parked). `request_panel_on 0` |
| 21.18 | **IMU stream starts by itself**: EP 0x83, 500 pkt/s exactly (2,500 per 5 s), `53 31 08 00 ...` |
| 23.28-23.79 | bridge log: `ept_init_rtc_timeout`, `etp_ld ld_en:1 ld_s:161 app_unlock:0`, `etp chip act_s:1 ept_s:1 app_s:0` |
| 23.1-52.8 | **total silence** on both logs; only 0x37 notifications and the IMU. No EP 0x82 data (controllers not attached), no panel, no camera video, no eye module on the bus |

## 3. Cross-reference: host command -> firmware reaction (time = pcap t)

| t | Host -> device | Firmware log that follows |
|---|---|---|
| 52.799 | `1a/5 [01 00 00]` | bridge: `eSpiCmdVideoCtrl cmd 1`, `video set work mode 0`; HMD 53.10: `usr_camera_resume`, `camera not power on, need init`, `eSpiCmdPcHeartbeat notify pc state is switched to alive`; 53.18-53.41 4x OV7251 power-up/init/probe, `start_fsin_pwm b_60hz 1`; bridge 53.05 `sony_panel_power_on`, `dp_power_on`, `lt7911uxe_power_on`; 53.36 `device manager old:0 pc_alive:1`, `fan wakeup`; 54.09-54.11 LT7911UXE chip_id 0x2204 name 0x1207 fw 0x0206; state DISPLAY_STATE_WAIT_PC_ALIVE -> WAIT_VIDEO_STABLE |
| 52.854-52.93 | identity reads: `16/0`, `03/0`, `09/0`, `50/0`, `19/0`, `03/5`, `03/4`, `56/5 [0a]`, `56/5 [0b 00 00 01]`, `03/6`, `56/5 [0b 02 00 01]` | `xr2_cmd_readSNmain`, `eSpiCmdSoftVer`, `eSpiCmdHwVer`, `eSpiCmdReadImuConfig`, `eSpiCmdDeviceCheckResult`, `eSpiCmdencryptr` |
| 52.875 | first `39/0 [00 00]` heartbeat, then every ~213 ms | `pc_heartbeat_timeout` never fires |
| 57.447-57.52 | `03/6`, `1b/5 [01]`, `39/0`, `27/5 'lens_rd'` | `eSpiGetVideoCtrlInfo cmd 1 return 1 10 0 0 0` |
| 57.795-57.99 | 84x `0a/0 [01 addr 10]` | `eSpiCmdReadDocument _data_type 1, _addr 0..1328, _data_len 16` (and log overflow, "Logs dropped") |
| 58.046 | `5b/5 [02 02]` | bridge: `ept_w_s write cmd:2 val:2`, `etp_ld ... app_unlock:1`, `etp chip app_s:1` |
| 58.047 | `29/0 [01]` | (log lost) |
| 58.268 | `03/0`, `1a/5 [02 48 00..]` | bridge: `eSpiCmdVideoCtrl cmd 2`, `open video prepare`, `requiredFps 72`, `keep upscale mode: 0`; HMD 58.565 `notify required_video_rate 72` |
| 58.408 | `1a/5 [05 00 ..]` | bridge: `use GPU cfg: Nvidia` |
| 58.41 | `39/0 [48 00]` from now on | |
| 58.78-60.3 | status polls `03/4`, `56/5 [03 07 06 05]`, `03/5`, `03/6` (every 0.5 s, round robin) | `etp_sts act_s sn_check:0 app_s:1 act_s:1` etc. |
| 59.851 | first `51/0` (then every 80 ms) | HMD 59.91-60.18: second camera init, `update_fsin_pwm current_hz 50`, `start_fsin_pwm b_60hz 0` |
| ~60.50 | (no USB: Windows started scanning out 72 Hz on the new DP monitor) | bridge: `calc fps 0 => 72`, `lt7911uxe_dp_lane_error_clear`, then `fps calc 72 cur 72 ... hitCnt stable/un N/N` every 0.5 s |
| 60.816-60.92 | `0a/5` x10 (ept record read), `27/0 'BildVer'`, `2c/0`, `27/0 'curfms'`, `27/0 'pbrg'`, `3a/5 [00]` | `ept_r_ept base_sn:P511X0X000000000001`, `ept_vef_sig in scr_len:35 key_id:1`, `eSpiCmdGetVisorStatus`, `eSpiCmdVisionSts` |
| 61.645 | `3c/5 [00 01]` | bridge `eSpiCmdeyetkctrl: [0]`; eye module (2104:0561) enumerates at t=63.25 |
| 65.937-65.98 | `55/5` x6 (KK=02, activation 293 B) | `ept_hid encpt_actv size:[293]`, `ept_act ... act_s:1 tm:1791867962`, `ept_vef_sig in scr_len:256 key_id:1`, `ept_vef_sig sha256 sus` (success) |
| (no USB) | | 65.999 `wait_video_info_stable ... fps_stable_hit_cnt > 10` (11 stable samples), 66.002 `lt7911uxe_set_mipi_enabled mipitx enabled: 1`, 66.608 `nvt check panel ready`, `panel change to WORK_WELL`, 66.617 `DISPLAY_STATE_WORK_ON`, 66.711 `nvt_set_bl_brightness 239`, 67.205 `Panel ON`, 68.32 audio amps (`aw883xx` x2, codec i2c writes), `codec_pa_cmd 0x2` |
| 67.271-67.307 | slot 2: `16/2 [1e]`, `50/2`, `0a/2 [00 00 00 10]`, `03/2` | HMD 68.43 `cmd 22 opt 2`, `eSpiCmdReadImuConfig`, `cmd 10 opt 2`, `cmd 3 opt 2`; **EP 0x82 reports start at t=67.271** (slot 2) |
| 68.997-69.09 | `56/5 [01]`, `5b/5 [01 64]`, `55/5` x8 (KK=01, 401 B) | `ept_hid encpt_ept size:[401]`, `ept_eqt sn:... exp_len:100`, `ept_vef_sig sha256 sus`, `stm_ept_status_set:1 ret:1`, `etp task` |
| 69.288-69.41 | slot 3: `16/3`, `50/3`, `0a/3`, `03/3` | EP 0x82 slot 3 reports start at t=69.288 |
| 70.52 onward | `51/0` @12.5 Hz, `39/0` @4.7 Hz | steady state |

## 4. Ordered host -> device transfer list (everything after Play started)

(Only commands in this list matter. Anything not listed was NOT sent. "IN poll" = Windows always keeps one INT-IN request pending on 0x81/0x82/0x83/0x84 of the headset and 0x81 of the bridge; those are not commands.)

### Phase A, Windows services open the device (t=52.45-52.80)
1. HID SET_IDLE(0) on dev 3 (unrelated MSI lighting device) at 52.458; string-descriptor reads (manufacturer / product / serial, idx 1-3, langid 0x0409, wLength 514) on headset (26), bridge (28), plus unrelated devs 3 and 9; repeated every few seconds by the Pimax service (harmless). No vendor requests.

### Phase B, headset command channel (dev 26, IF0 EP 0x01 OUT) in order
See sections 1 and 3: `1a/5[01]`, identity reads, `39/0` heartbeat loop, `1b/5`, `27/5 lens_rd`, 84x `0a/0` (calibration), `5b/5[02 02]`, `29/0[01]`, `1a/5[02 48]`, `1a/5[05 00]`, `39/0[48 00]`, status poll loop `03/4 56/5 03/5 03/6`, `51/0` loop, `0a/5` ept record, `27/0` keys, `2c/0`, `3a/5`, `3c/5[00 01]`, `55/5`xN, `16/2 50/2 0a/2 03/2`, `56/5[01] 5b/5[01 64] 55/5`xN, `16/3 50/3 0a/3 03/3`.

### Phase C, camera (dev 29, 34A4:0014, USB3 UVC) t=57.87-57.96, **all standard UVC**
| t | bmReqType/bReq | wValue / wIndex / wLength | data | meaning |
|---|---|---|---|---|
| 57.873 | 01 / 03 SET_FEATURE | 0000 / 0000 | - | USB3 FUNCTION_SUSPEND resume (Windows power mgmt; Linux does it itself; the same toggle `wIndex 0100`/`0000` occurred 3x at t=21.4-21.5 and 26.6) |
| 57.895 | a1 / 81 GET_CUR | 0100 PROBE / 0001 / 26 | `0000 01 01 00000000 0000 0000 0000 0000 0000 00960000 00080000` (interval 0, dwMaxVideoFrameSize 38400, dwMaxPayloadTransferSize 2048: Windows' stale default) | default probe |
| 57.896 | 21 / 01 SET_CUR | 0100 PROBE / 0001 / 26 | `0000 01 01 0a8b0200 0000 0000 0000 0000 0000 00960000 00000000` (hint 0, fmt 1, frame 1, dwFrameInterval 0x028b0a = 166666 = 60 fps, size field still 38400, payload 0) | negotiate |
| 57.916 | a1 / 81 GET_CUR, a1 / 83 GET_MAX, a1 / 82 GET_MIN | same | `... 80200000 00800000` (dwMaxVideoFrameSize 2,129,920; dwMaxPayloadTransferSize 32768) | |
| 57.924 | 21 / 01 SET_CUR PROBE | | same with payload field 0 | |
| 57.935 | a1 / 81 GET_CUR PROBE | | confirms 2,129,920 / 32768 | |
| 57.935 | 21 / 01 SET_CUR **COMMIT** | 0200 / 0001 / 26 | fmt 1, frame 1, interval 166666, 2129920, 32768 | |
| 57.954 | 00 / 0b SET_INTERFACE | wValue 9 (alt 9) / wIndex 1 | - | iso IN EP 0x81, SS companion: maxburst 15, mult 0, 32768 B/service interval, wMaxPacketSize 0xfc00 |
| 57.957 -> end | iso IN EP 0x81 | | ~62.5 URB/s, ~1.60 MB each (= the 1280x832 YUY2 frame, 2,129,920 B) | frames flow |
**No UVC extension-unit SET_CUR/GET_CUR, no brightness/exposure control, no other alt setting, no further PROBE/COMMIT.** The XU (adccb1c2-..., selectors 2,3,10,11,12) was only probed (GET_LEN/INFO/MIN/MAX/RES/DEF) at enumeration (t=21.4). Camera streaming does not need XU access. Frame interval 166666 (60 fps) was requested. The headset-side prerequisite is the `1a/5[01]` "PC alive" (sensors powered). Whether frames carried valid pixels before the `0x51` push began (t=59.85) was not checked (snaplen 512).

### Phase D, bridge 34A4:0043 (dev 28)
Host sent nothing except standard enumeration (SET_CONFIG, SET_IDLE(0), report descriptor, string descriptors). It only reads EP 0x81 (ASCII log). Its functions are driven through `opt 5` commands on the headset channel.

### Phase E, controllers (via headset channel; no EP 0x82 command channel, no host command to start the stream beyond phase B)
Per slot (2 = left, 3 = right), within ~50 ms, in this order (waiting for each reply is not required; Windows pipelines):
1. `58 04 16 SS 1e` (read serial)
2. `58 03 50 SS` (IMU config; controllers report 0x01f4 = 500 Hz)
3. `58 07 0a SS 00 00 00 10` (read radio id; Windows sent this twice for slot 3)
4. `58 03 03 SS` (fw version)
After this, `53 32 08 SS 43 ...` reports flow on EP 0x82 immediately (first report 0.4 ms after command 1 for slot 2: t=67.2711; 2.5-5 ms intervals). Controllers were already connected and being announced via 0x37 before; no `0x04 StartPair`.

### Phase F, eye module (dev 30, VID 2104 PID 0561 "Tobii", appears only after `3c/5[00 01]`)
Enumeration at t=63.25: device desc `12 01 00 02 00 00 00 40 04 21 61 05 00 01 01 02 03 01` (bcdUSB 2.0, 1 config, 67 B), config: one interface IF0, class ff/00/00, 3 bulk endpoints: EP 0x83 IN 512, EP 0x04 OUT 512 (never used), EP 0x05 OUT 512 (config total 67 B; a vendor descriptor `1c ff "BIST" ...` precedes the interface). String 3 serial `XR5DA-P511X0X000000000001`. SET_CONFIG 1.
Then (all from t=66.77):
| t | transfer | content |
|---|---|---|
| 66.7756 | ctrl OUT bm 0x41 req 0x30, wLen 24 | `01000000 00000000 01000000 04000000 00000000 00000000` |
| 66.7758 | ctrl IN bm 0xc1 req 0x30, wLen 512 | 484 B: header `01000000 01000000 01000000 04000000 00000000 97000000` + text `/fw/upgradeprotocolversion=1\n/hw/platformtype=XR5EYECHIP\n/fw/t2serverversion=df9b4b881f\n/hw/subplatformtype=XR5PIMAXDA\n/fw/fwversion=df9b4b881f-7.32.0` |
| 66.7762 | ctrl IN bm 0xc1 req 0x46 wValue 1 wLen 8 | module microsecond clock (u64 LE: 0x4d2ab2 ...), polled repeatedly (time sync) |
| 66.7768 | ctrl OUT bm 0x41 req 0x41, no data | start of TTP session |
| 66.777 .. 66.798 | bulk OUT EP 0x05 / bulk IN EP 0x83: 19 TTP requests (section 5) | |

## 5. Eye module TTP request list (wrapper `u32 LE dir(0 out/1 in) | u32 LE len`, header 24 B big-endian `magic(0x51 req, 0x52 resp, 0x53 notify) | seq | flags(resp=1) | opcode | 0 | payload_size`, payload TLV with a 2-byte prefix `00 00`, then `tag u8 | len u32 BE | value`)
| seq | opcode | payload size | meaning (L unless noted) |
|---|---|---|---|
| 1 | 0x3e8 | 47 | hello / capability list: `0000 17 00000028 ...` ids 0..8 (reply 11 B: `02 00000004 00010008`) |
| 2 | 0x640 | 0 | get (reply 29 B, three zero u32) |
| 3 | 0x76c | 24 | **authentication step 1**: `02 4 00002712 | 17 8 00000001 00000000`. Reply carries a **256-byte blob** (tag 0x15 len 0x104 -> `00000100` + 256 B random-looking, begins `e27e124dc1ea1554...`): challenge/public data |
| 4 | 0x777 | 58 | **authentication step 2**: `02 4 00002712 | 02 4 00000000 | 15 14 00000010 <16 B: 81c09c494f536c08bedd9416f0d8ae38> | 15 8 00000004 816d4df2`. 16-byte response + 4-byte tag; reply empty (success). Bytes cannot be replayed verbatim if the challenge is fresh per session (not verifiable: single session captured). **Main blocker for Linux eye tracking** |
| 5 | 0x852 | 2771 | big config upload (calibration/geometry blob: S15.16 fixed-point rows, e.g. `ffffbedc ac000000 ...`); reply empty |
| 6 | 0x77b | 11 | `02 4 00002712` (finish auth?) |
| 7 | 0x532 | 0 | get properties (reply 267 B contains strings `HMD`, `XR5EYECHIP`, `XR5PIMAXDA`, `false`) |
| 8 | 0x58c | 0 | get ids (reply: `XR5DA-P511X0X000000000001`, `XR5_PIMAX_DA`, `XR5`, `df9b4b881f-7.32.0`) |
| 9, 10, 11 | 0x546, 0x547, 0x5d2 | 0 | get settings dumps (460 / 516 / 275 B replies, key/value TLVs `true/false`) |
| 12 | 0x5cd | 0 | get (reply `...00000001`) |
| 13 | 0x4b0 | 0 | get feature list (replies include `image`, `wearable`, `image_collection`...) |
| 14, 15 | 0x596, 0x578 | 0 | get (all-zero u64 tables) |
| 16 | 0x686 | 0 | get (rates table: 0x1e, 0x32, 0x3c, 0x5a, 0x64, 0x78 = 30/50/60/90/100/120 Hz gaze rates; current 0x78 = 120) |
| 17 | 0x67c | 29 | set rate: `05 4 00022af8 | 02 4 00000078 | 02 4 00000078` (120 Hz) |
| 18 | 0x5be | 98 | set (u32 pairs, zeros) |
| 19 | 0x672 | 0 | get rate (reply echoes 0x78) |
| 20 | 0x4c4 | 20 | `02 4 00000505 | 17 4 00000000` = **subscribe to stream 0x0505 (gaze)**(M) |
| 21 | 0x456 | 34,612 | **upload user eye-calibration profile** named `human` (sent as 4087-byte USB bulk pieces x8 = 34,612 B). Host-held data; reply empty at t=67.2577 |
| (notify) | 0x4ba stream 0x0505 | 1210 | **gaze stream starts at t=67.2611**, 120.5 Hz (8.3 ms), 1242 B notifications as in eyetracking.md |
| 22 | 0x44c | 0 | (t=68.27) download the stored profile back (34,612 B) |
| 23 | 0x5be | 98 | refresh (t=68.35) |
Poll `GET_DESCRIPTOR string 3` and vendor `0xc1/0x46` clock reads continue. Calibration cycles (0x3f2/0x407/...) are NOT part of start-up.

## 6. Headset calibration blob (new)
`0a/0` document type 1: bytes 0..3 = u32 LE length 1318 (0x0526), bytes 4..15 zero, bytes 16.. = zlib stream (`78 da`) of 1318 bytes -> **3410-byte XML** `<DeviceConfiguration deviceUID="P511X0X000000000001" cameraModel="ov7251" pimax_calibration_version="Pimax_Calibration_tag_0.3" calibration_date=...>` with 4 `<Camera>` (trackingA, trackingB, Pi-controllerA, Pi-controllerB: size 640x480, principal point, focal ~230-235 px, FISHEYE_4_PARAMETERS radial distortion, rig translation + rowMajorRotationMat) and `<SFConfig><Stateinit ombc tbc aBias wBias ka kg na ng ombg ...>` (IMU bias, scale, non-orthogonality). Saved verbatim as `headset-calibration.xml`. Linux reader: send `58 07 0a 00 01 AL AH 10` for AL:AH = 0x0000, 0x0010, ... , 0x0530 (84 reads), each reply `4c 17 0a 00 01 AL AH 10 <16 B> CK`, concatenate the 16-byte payloads, inflate `blob[16:16+u32(blob[0:4])]`.

## 7. Replay candidate (minimal, to be validated on Linux)

Legend: HID = hidraw write of exactly 64 bytes (pad with zeros; hidraw has no report ID byte because the descriptor has none, so the first byte written is 0x58) to IF0 / EP 0x01 of 34A4:0044; "wait" = read the matching `4c` reply on IF0 (hidraw read) before continuing. Reads on IF2 (IMU) and IF3 (log) should be started first and kept running.

| # | Device | Transfer | Endpoint / request | Hex payload (first bytes; rest 0) | Wait reply? | Goal |
|---|---|---|---|---|---|---|
| 0 | HMD | open IF0..IF3 hidraw; SET_IDLE(0) per interface (Linux hid does it) | - | - | - | IMU already streaming (EP 0x83); log on EP 0x84 |
| 1 | HMD | HID out | IF0 EP 0x01 | `58 06 1a 05 01 00 00` | yes `4c 04 1a 05 01 07` | **wake**: PC-alive. Starts sensors, powers panel/bridge, DP receiver. Wait ~1 s: the DisplayPort connector/EDID appears |
| 2 | HMD | HID out loop | IF0 EP 0x01 | `58 04 39 00 00 00` every 213 ms (no reply) | no | keepalive (must be < 5 s). Replace payload by `58 04 39 00 48 00` after step 5 |
| 3 | HMD | optional reads | IF0 | `58 04 16 00 1e` ; `58 03 03 00` ; `58 03 09 00` ; `58 03 50 00` | yes | identity/versions (not shown to be necessary) |
| 4 | HMD | optional calibration | IF0 | 84x `58 07 0a 00 01 AL AH 10` | yes each (2 ms) | read XML (section 6) |
| 5 | HMD | HID out | IF0 | `58 05 5b 05 02 02` | yes `4c 05 5b 05 02 01 2d` | ept `app_unlock` (may be needed for video; Play does it before video; safe) |
| 6 | HMD | HID out | IF0 | `58 04 29 00 01` | yes `4c 04 29 00 01 07` | panel-on request (M) |
| 7 | HMD | HID out | IF0 | `58 0d 1a 05 02 48` | yes ack | announce 72 Hz video |
| 8 | HMD | HID out | IF0 | `58 0d 1a 05 05 00` | yes ack | GPU cfg (Nvidia = 0; try 0 first, other values untested) |
| 9 | host GPU | **Linux DRM**: output the headset DP connector at the mode that gives 72.0 Hz (the mode that Windows chose; panel native mode unknown here, check the EDID/DisplayID of the new connector, 72 Hz entry, no DSC info in USB) | - | - | - | firmware needs 11 stable frame-rate samples (0.5 s each) then it enables MIPI and panel |
| 10 | HMD | HID out loop | IF0 | `58 04 39 00 48 00` every 213 ms | no | keepalive with rate |
| 11 | CAM 34A4:0014 | UVC | PROBE SET_CUR (fmt 1, frame 1, interval 166666) / GET_CUR / COMMIT / SET_INTERFACE IF1 alt 9 | see Phase C | yes | video stream (uvcvideo does this on open; request 1280x832 YUY2 @ 60 fps) |
| 12 | HMD | HID out loop | IF0 | `58 19 51 00 01 32 98 08 98 08 98 08 98 08 0d 00 0d 00 0d 00 0d 00 28 00 3e 00` every 80 ms | optional (ack `4c 04 51 00 01 07`) | camera sync/exposure push (values vary, see controllers-settings.md 1.2; start with a constant) |
| 13 | HMD | HID out | IF0 | `58 04 16 02 1e` ; `58 03 50 02` ; `58 07 0a 02 00 00 00 10` ; `58 03 03 02` (then the same with slot 03) | yes | **start controllers**: EP 0x82 reports start |
| 14 | HMD | HID out | IF0 | `58 05 3c 05 00 01` | yes `4c 04 3c 05 00 00` | power the eye module; wait for USB enumeration of 2104:0561 (about 1.6 s) |
| 15 | EYE | ctrl OUT 0x41/0x30 then IN 0xc1/0x30 (512), 0xc1/0x46 (clock), OUT 0x41/0x41 | EP0 | `01000000 00000000 01000000 04000000 00000000 00000000` | yes | open session |
| 16 | EYE | bulk OUT EP 0x05 / IN EP 0x83 | TTP requests seq 1..21 (section 5) | - | yes | needs authentication (0x76c/0x777) + profile (0x456) + subscribe (0x4c4); then 1242 B notifications at 120 Hz |
| 17 | HMD | ept handshake `5b/5 [01 64]`, `55/5` chunks, `56/5` polls | IF0 | - | yes | (see 7.1) probably not needed |

7.1 Open question: the activation/ept protocol (0x55/0x5b/0x56). The 293-byte activation (`encpt_actv`) and 401-byte (`encpt_ept`) blobs are generated by the Pimax service (they embed the serial and the current Unix time `tm:1791867962` = capture time + signature verified by the firmware). The firmware already has `act_s:1 ept_s:1` stored from before (boot log t=23.79), so the panel/camera/IMU/controllers are not gated by it in this capture (video was stable at t=66.0 BEFORE the second activation upload at t=69.0, but AFTER the first upload at t=65.94, so the first upload could be the gate; the same second as `fps_stable_hit_cnt > 10`). To resolve: on Linux skip it first (steps 5/17 omitted): if the panel stays off after 11 s of stable 72 Hz video, the activation upload (replay of these exact blobs with the serial and time; may need a fresh signature) is the missing piece. The firmware validates `tm` (printed as `[1787925078][1791867963][2145916800]` = bounds + now), so a replayed blob likely works only within a validity window.

## 8. Anything on DisplayPort paths (goal 4)
* No USB command carries mode, refresh, DSC or EDID data. The only display-related host input is: `1a/5 [02 48 ..]` = required fps 72 (Hz), `1a/5 [05 00 ..]` = GPU vendor (Nvidia), `39 [48 00]` = rate in keepalive, and the `27/5 lens_rd` read.
* Bridge log lines: `lt7911uxe_check_chip_id chip_id: 0x2204`, `chip_name: 0x1207`, `FW version: 0x0206`; `[E] vxr_check_dsc_mode, to do` (DSC check not implemented in this firmware: the link is plain, not DSC, as far as the log shows); `[E] dp_rx_timming_check, to do`; `lt7911uxe_dp_lane_error_clear`; `calc fps 0 => 72` then `fps calc 72 cur 72 ... err(8000-8000-8000-8000)` (frame time measured per 4 panel tiles/lanes, all equal); MIPI TX 4 ports `new mipi port cap [0xA3]x4`; `nvt_set_bl_brightness 239`; `panel_locldimming_level_set, to do` (local dimming not implemented). `[sony_panel_power_on]`, `nvt check panel ready` -> a Sony panel with Novatek driver.
* Display ids from `3a/5` log: `displayid:2 lensid:0 boardid:3 lighthouse:0 sku:1`.
* Resolution/EDID is not on USB; read it from the Linux DRM connector after step 1. Windows picked 72 Hz; the user's settings capture changed refresh to 90 (see controllers-settings.md; no USB command was seen for it, so refresh is set purely by the display mode).
* The firmware measures the video: if the Linux mode is not 72.0 Hz (within tolerance) `fps_stable_hit_cnt` will not reach 11 and the panel stays off (M).

## 9. Confidence / what Linux needs
Needed for display + sensors: steps 0-2, 7-10 (plus 5 and 6 until proven unnecessary), exact 72 Hz DRM mode. IMU needs nothing. Cameras need step 1 and standard UVC (step 11). Controllers need step 13 only. Eye needs steps 14-16 including a working authentication (open).
Overall: H for the command framing and the wake/keepalive/video-announce chain, M for panel-on ordering and the need for activation, L for the unknown payload fields (0x51 values, version encodings, 0x19, 0x2c, trailing CK).
