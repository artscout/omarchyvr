# Windows vs Linux host commands: what could select left/right routing (both panels show LEFT half)

Read-only analysis, 2026-10-13. No device access.
Sources: all `58 LL CMD OPT data` OUT reports on EP 0x01 in captures/*hub2 and captures2/*hub2 (re-extracted with analysis/tools/usbp.py,
11,193 reports; `tshark -Y usb.capdata` returns 0 packets for the two cold-start pcaps, so use usbp.py there), plus
`pimax-data/LocalAppData-Pimax/FirmWare/p3c_stm32_*.txt` (40 bridge sessions), `p3c_mcu_*.txt` (40 headset-MCU sessions), analysis/cold-start-fwlog.txt.
Confidence: H = seen in bytes/logs, M = consistent, L = inference.

## 0. Bottom line

1. **Windows sends no host command that mentions or selects eye/stereo/mirror/split/port/swap/3D.** The words eye, stereo, mirror, dual, split, swap, 3d,
   left, right never appear in any bridge (p3c_stm32) or MCU log. Only hits: `eSpiCmdeyetkctrl` (Tobii eye-tracker power), `dp_lane_error_clear`,
   `mipi port cap`, `mipitx enabled` and DSC/PPS stubs (H).
2. **Every display-state-changing write Windows sends before Panel ON is also sent by Linux with identical bytes** (`1a/5 [01 00 00]`, `5b/5 [02 02]`,
   `29/0 [01]`, `1a/5 [02 48 00..]`, `1a/5 [05 00..]`, `39/0 [48 00]`). Differences are only reads, camera/eye/ept traffic (section 1) (H).
3. The bridge firmware never logs a resolution, DSC slice count, PPS, lane count, routing or "mode". `new mipi port cap: [0xA3]x4` and `mipitx enabled: 1`
   are identical in Windows and Linux. The L/R split is therefore not configured through any logged host command; it is fixed in the LT7911UXE
   (splits by pixel/slice position) or depends on **what the GPU sends** (section 5). Most likely cause is on the DP/DSC/scanout side, not a missing command (M).

## 1. Distinct host->device commands Windows sends (all captures; opt: 0 HMD MCU, 2/3 controllers, 5 display bridge)

"Linux?" = we send it today (keepalive service + pimax_video_on.py + controller bursts + calibration reads). t = seconds into that pcap.

| cmd/opt | payload (hex) | first seen (pcap, t) | Firmware reaction (log line) | Linux? |
|---|---|---|---|---|
| 1a/5 | `01 00 00` | cold-start 52.80 | bridge `eSpiCmdVideoCtrl cmd 1`, `video set work mode 0`, `sony_panel_power_on`, `lt7911uxe_power_on`; HMD `pc state is switched to alive`, cameras init | yes (`58 06 1a 05 01 00 00`) |
| 1a/5 | `02 48 00 00 00 00 00 00 00 00` (72 Hz) | cold-start 58.27 | bridge `open video prepare`, `requiredFps 72`, `keep upscale mode: 0`; HMD `required_video_rate 72` | yes (`58 0d 1a 05 02 48`) |
| 1a/5 | `02 5a 00..` (90 Hz) | settings 92.77 | `requiredFps 90` | n/a (72 only) |
| 1a/5 | `05 00 00..` | cold-start 58.41 | `use GPU cfg: Nvidia` | yes |
| 1a/5 | `04` | poweron 7.85 | `required_video_rate 0` / video close | not needed |
| 29/0 | `01` / `00` | shutdown 1.55/15.65; cold 58.05 | MCU `eSpiCmdPanelOnReq data 1`, `request_panel_on state changed 0->1` | yes (`01`) |
| 39/0 | `RR 00` (00, then 48; 5a at 90 Hz) every ~213 ms | all | none (`pc_heartbeat_timeout` if absent) | yes |
| 5b/5 | `02 02` | cold 58.05 | `ept_w_s write cmd:2 val:2`, `etp_ld ... app_unlock:1`, `app_s:1` | yes |
| 5b/5 | `01 64` (before 0x55 KK=01) | cold 69.01 | `ept_w_s write cmd:1 len:100 max:100` | **no** |
| 55/5 | `02 00..05` x6 (293 B activation blob) | cold 65.94 | `ept_hid encpt_actv size:[293]`, `ept_act ... act_s:1 tm:<unix time>`, `ept_vef_sig sha256 sus` | **no** |
| 55/5 | `01 00..07` x8 (401 B ept blob) | cold 69.01 | `ept_hid encpt_ept size:[401]`, `ept_eqt ... exp_len:100`, `stm_ept_status_set:1 ret:1`, `etp task` | **no** |
| 56/5 | `0a`, `0b 00 00 01`, `0b 02 00 01`, `03/07/06/05`, `01`, `00`, `0b 00 00 20` | cold 52.87.. | `eSpiCmdencryptr`, `etp_sts exp_fw:2 / act_s:1 / ept_s:1 force:1`, `ept_r_ept expand:..` | **no** (status reads) |
| 1b/5 | `01` (reply `01 0a 00 00 00`) | cold 57.45 | `eSpiGetVideoCtrlInfo cmd 1 return 1 10 0 0 0` | **no** (read) |
| 27/5 | key `lens_rd` (reply `0f 0f 00 00 00 05 00 99 1d 99 1d f6 ff f6 ff 01`) | cold 57.51 | `cmd 39 opt 5 datalen 8` | **no** (read) |
| 27/0 | keys `BildVer`, `curfms` (`01 00`), `pbrg` (`01 5e`) | cold 60.83 | MCU key/value get | no (reads) |
| 26/0 + 2a/0 | `pbrg 01 PCT`; `2a PCT` | ipd-brightness 7.37 | `eSpiCmdPanelBrightnessAdj`, `set backlight brightness` | no (user setting) |
| 3a/5 | `00` (reply `00 12 7e`) | cold 60.86 | `eSpiCmdVisionSts: displayid:0x2 lensid:0 boardid:0x3 lighthouse:0 sku:0x1` | **no** (read) |
| 2c/0 | `00` (reply `1c 54`) | cold 60.83 | `eSpiCmdGetVisorStatus` | no (read) |
| 3c/5 | `00 01` | cold 61.65 | bridge `eSpiCmdeyetkctrl: [0]`; Tobii 2104:0561 appears 1.6 s later | **no** |
| 51/0 | `01 32 AA AA BB BB 0d 00 0d 00 0d 00 0d 00 28 00 3e 00` @12.5 Hz | cold 59.85 | HMD `update_fsin_pwm current_hz 50` (camera sync) | no |
| 5c/5 | `01 lo hi` (IPD mm*100, `17 70`=60.00 mm) | settings 18.37 | `eSpiCmdSetIPD`, `motor_ipd_set type:1` | no (user setting; absent at cold start) |
| 03/{0,4,5,6,9,2,3}, 09/0, 19/0, 50/{0,2,3}, 16/{0,2,3}, 0a/{0,5,2,3}, 04/0 | identity / version / calibration / controller reads | cold 52.85..60.9 | `eSpiCmdSoftVer/HwVer/DeviceCheckResult/ReadImuConfig/ReadDocument`, `xr2_cmd_readSNmain` | partly (0a/0 calibration and controller bursts yes; `03/4,5,6,9`, `09`, `19`, `0a/5` no) |

In firmware logs but NEVER in any USB capture (sent in other Windows sessions, bytes unknown, only dispatch lines exist):
- `cmd 90 opt 5 datalen 1` = `eSpiCmdSetUpscaleMode`, always prints `keep upscale mode: 0`. Only in the 8 sessions using `video set work mode 1`
  (90 Hz / no-required_fps path: p3c_stm32_26-10-03-11-10-46, -11-11-03, -11-13-19, -11-34-08, 10-04-09-30-19, 10-10-23-47-01, 10-11-08-26-51, 10-11-09-21-30).
- `cmd 26 opt 5 datalen 3` with `work mode 1` (same 8 sessions). Captured work mode 0 is `01 00 00` (Linux sends the same).
- `cmd 43 opt 0 datalen 2` `eSpiCmdVisorCtl data 1` (2 MCU sessions), `cmd 40` `eSpiCmdSysReset`, `cmd 26 opt 5 datalen 1` (video reset/close), `cmd 6 opt 2/3`
  (controller), `cmd 85 opt 5 datalen 5`. None is display-routing related by name.

## 2. Firmware log keyword sweep (both Windows log sets + cold-start-fwlog)

Keywords: eye, stereo, mirror, dual, split, port, left, right, swap, 3d, panel mode, mipi, dsc, pps, lane, lcd, half, side, upscale, scaler, vision.
All distinct matching bridge messages (counts over 40 sessions): `mipitx enabled: N` (3599), `new mipi port cap: [0xA3]x4` (440, identical every time),
`nvt_set/lock_bl_brightness`, `eSpiCmdeyetkctrl: [0]` (64), `dp_lane_error_clear`, `sony_panel_power_on/off`, `vxr_check_dsc_mode, to do` (stub),
`dp_rx_timming_check, to do` (stub), `eSpiCmdVisionSts displayid:[0x2] lensid:[0x0] boardid:[0x3] lighthouse:[0x0] sku:[0x1]` (39, always identical),
`mipi write failed / check AB timeout` (3, panel_wakeup glitch), `panel_locldimming_level_set, to do`.
No resolution, h_act, slice, PPS, bpp, lane count or routing text exists. MCU log adds only `request_panel_on`, `set_indis_from_stm32 release s_new:0x440/0x240/0x40`
(status bits), codec/Awinic audio, camera init. Nothing about eyes or ports.

**Activation (0x55/0x56/0x5b) effect:** the only values ever logged are `ept_s:1`, `act_s:1`, `app_s:1`, `exp_fw:2`, `force:1` (two boots show `app_unlock:0`/`app_s:0`
until `5b/5 [02 02]` flips them to 1). The 293 B blob only logs `act_s:1 tm:<now>` + `sha256 sus`. Nothing display-related changes afterwards: in the reference session
(p3c_stm32_26-10-11-09-45-07 L205-270) `mipitx enabled: 1` prints at 4747030, THEN Windows uploads the 401 B ept blob (4747404-4747511), and `nvt check panel ready` /
`WORK_ON` / `Panel ON` follow. In the cold start the chip already reports `act_s:1 ept_s:1` at boot with no host command (23.3 s). So 0x55/0x56/0x5b[01 64] look like
licence/anti-clone handshakes, not a panel-mode switch (M-H).

## 3. Panel-ON sequence diff (bridge log)

| Step | Windows (p3c_stm32_26-10-11-09-45-07, 72 Hz) | Linux (your summary) |
|---|---|---|
| video prepare | `requiredFps 72`, `keep upscale mode: 0`, `use GPU cfg: Nvidia` | same |
| fps | `calc fps 0 => 72`, `fps calc 72 cur 72 last 0, err(8000-8000-8000-8000) hitCnt 11/11` | same err values |
| mipi | `mipitx enabled: 1` -> `nvt check panel ready` -> `WORK_ON` -> `new mipi port cap: [0xA3]x4` -> `backlight 211` -> `Panel ON` | same |
| Windows-only lines | `eSpiGetVideoCtrlInfo ... return 1 10 0 0 0`, `eSpiCmdVisionSts`, `eSpiCmdeyetkctrl`, `ept_*`/`etp_*` (3 blobs), `codec_pa_cmd 0x2` after Panel ON | not sent |

There is no Linux bridge log file in the repo (only your summary), so the diff covers only the lines you listed. No Windows-only line is a plausible routing step.

## 4. Candidates ranked (chance the missing/different item changes L/R routing)

No candidate has direct evidence. Ranked by "closest thing to a mode selector the firmware exposes".
1. **`cmd 90 SetUpscaleMode` (0x5a/opt 5, 1 data byte) and the upscale byte in `1a/5 [02 RR UU ..]`.** Only "mode" setter besides work mode. Bridge prints `keep upscale mode: 0`
   for both. Windows `02 48 00..` has UU=00 and Linux sends the same, so it already matches. Cmd 90 payload never seen on USB. Rank: low-medium.
2. **`1a/5 [01 WM xx]` work mode.** Windows logs work mode 0 (captured `01 00 00`, 72 Hz, `open video prepare` path = Linux) and work mode 1 (8 sessions, 90 Hz path with cmd 90,
   no required_fps, payload never captured; different bridge path). Linux already uses the mode Windows uses at 72 Hz. Rank: low.
3. **`3a/5 [00]` VisionSts (`00 12 7e` -> displayid 0x2, lensid 0, boardid 3, sku 1) and `27/5 lens_rd`.** Reads only; the host (Pimax service) uses them to pick panel/lens layout.
   Not a routing write. Relevant only to host-side composition (rotation, per-eye viewport). Rank: low.
4. **0x55 / 0x5b `01 64` / 0x56 activation.** Never changes display state in logs (section 2). Replay is infeasible (serial/time-bound, RSA check). Rank: low.
5. **0x3c/5 `00 01` (eye tracker power), 0x51 camera sync push, 0x5c IPD.** No link to the display path. IPD is a physical motor, absent from cold start. Rank: very low.

## 5. The more likely cause: the DP/DSC stream or scanout, not a command (L-M)

- Firmware only receives DSC slices over DP and forwards them via the LT7911UXE to 4 MIPI ports (`[0xA3]x4`); both panels are fed from one 7104-wide frame. Left half
  (slices 0-3 of 8, 888 px each per analysis/nvidia-dsc-linkrate.md) should go to the left pair, right half to the right pair.
- If both panels show the left half, check the GPU side first: (a) NVIDIA 2Head1OR (dual) mode: head 1 must scan out x=3552..7103; a DRM lease client (Monado) that sets plane
  SRC_X / CRTC_X or a framebuffer of width 3552 instead of 7104 duplicates the left half; (b) confirm the committed DRM framebuffer is 7104x3840 with the right-eye viewport at
  x=3552 (the windowed test proved compositor output only, not the lease scanout); (c) dump the live plane (`drm_info` / `modetest -p`) while Monado runs: SRC_W=7104, CRTC_W=7104.
- Windows DPCD/PPS (slice count, bpp) was never captured, so DSC differences vs Windows are unobserved (headset-dpcd.md). Sink advertises slices 1/2/4/8, max slice width 2560, 8.0 bpp.
- Cheap discriminator with no new command: scan out a left/right-distinct test pattern with a plain DRM modeset (not via Monado), e.g. `modetest -s <conn>:7104x3840@72` with a
  patterned FB. If both panels still show the left half, the bridge/DSC path is at fault (vary slice count/bpp via EDID DisplayID DSC hint); if correct, the fault is in Monado's plane config.

## 6. Reproduce

`python3 -I` with `analysis/tools/usbp.py` `read()`: iterate packets with `ep&0x80==0 and data[0]==0x58`, key on (cmd=data[2], opt=data[3], data[4:1+LL]).
Logs: `grep -aoE 'cmd [0-9]+ opt [0-9]+ datalen [0-9]+'` over p3c_stm32_*.txt / p3c_mcu_*.txt.
