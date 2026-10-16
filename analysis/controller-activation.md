# Controller activation (green LED + vibration, IR LEDs): what Windows sends that Linux does not

Offline analysis, 2026-10-14. Nothing was sent to the device. Sources: Windows USBPcap captures (`captures/`, `captures2/`, `pimax-data/windows-20261014/usb-captures/`, same files under `/mnt/shared/pimax-linux/windows/usb-captures/`), the headset MCU firmware logs on EP 0x84, the 83 `p3c_mcu_*.txt` MCU logs from the Windows install (`pimax-data/LocalAppData-Pimax/FirmWare/`), and the Linux code (`libpimax/src/pimax.c`, `linux/pimax_controllers.py`, Monado `pimax_ctrl.c`).
Confidence labels: H proven, M plausible, L guess.

## 1. Short answer

1. **No host command that lights the controller green, vibrates it, or enables IR LEDs exists in any Windows capture or log.** Every host->device transfer to the headset (34A4:0044) is on interrupt EP 0x01 (`58 LL CMD OPT ...`). Control transfers to it are only GET_DESCRIPTOR, SET_CONFIGURATION, SET_INTERFACE, HID SET_IDLE, UAC GET_CUR. There is **no HID SET_REPORT and no vendor request** (checked in cold1 and in the controllers capture). The eye module (2104:0561) and the bridge (34A4:0043) get nothing controller related.
2. The complete set of commands addressed to a controller (OPT 2 = left, 3 = right) is: `0x16`, `0x50`, `0x0a`, `0x03` (the "attach" burst) plus `0x04` StartPair (OPT 0, data = slot). The MCU logs of 83 Windows sessions list exactly these (`cmd 3/10/22/80 opt 2|3`, `cmd 4 opt 0`) and no other command with opt 2 or 3. Linux already sends all four attach commands.
3. Most likely the green light and the buzz are produced by the **controller firmware itself when its radio link to the headset comes up** (M, about 55 %). The only slot-addressed host command that changes link state is StartPair (about 20 % that it is the trigger).
4. Caveats: no capture contains a marker "controller went green / buzzed now"; no capture contains a game haptic pulse, so a haptic command could exist and simply never be exercised. A targeted Windows capture is needed to settle it (section 7).

## 2. What Linux sends today

`libpimax/src/pimax.c` (used by Monado via `pimax_ctrl.c`; `linux/pimax_controllers.py` is the same):

| Command | Bytes (64 B report, zero padded) | Note |
|---|---|---|
| PC alive | `58 06 1a 05 01 00 00` | at session start |
| keepalive | `58 04 39 00 RR 00` (RR = fps, 0x48/0x5a), every 213 ms | |
| app unlock | `58 05 5b 05 02 02` | `pimax_video_prepare` |
| panel-on request | `58 04 29 00 01` | |
| video prepare | `58 0d 1a 05 02 FPS ...`, `58 0d 1a 05 05 00 ...` | |
| controller attach, per slot SS | `58 04 16 SS 1e`, `58 03 50 SS`, `58 07 0a SS 00 00 00 10`, `58 03 03 SS`, 10 ms apart | identical to Windows |
| camera exposure | `58 19 51 00 01 32 ...` only with `PIMAX_CAM_AE=1` | approved, opt-in |

Monado `pimax_ctl_set_output` is a no-op ("haptics: protocol unknown"). Nothing is sent that targets LEDs or haptics, and nothing could be, because no such command is known.

Not sent on Linux (but sent by Windows in a cold start): `0x19` DeviceCheckResult, `0x09`, `0x1b`, `0x2c`, `0x3a`, `0x3c` eye enable, `0x27` key reads (`curfms`, `BildVer`, `lens_rd`), `0x56` status polls, and the `0x55` signed activation uploads (+ `0x5b [01 64]`).

## 3. Every distinct host->headset command in the Windows captures

Scan of all 20 `hub2` pcaps (EP 0x01 OUT, magic 0x58). Reply on EP 0x81 starts `4c`. "Rate" is for periodic ones.

| CMD OPT | Bytes (first 8 shown) | When | Reply | Controller related |
|---|---|---|---|---|
| 04 / 0 | `58 04 04 00 SS` (SS = 2 or 3) | only in `20261013-124629-controllers` at 41.19 (SS=2, nobody answered, 30 s timeout), 71.74 (SS=3), 121.52 (SS=2); 12 times in 83 MCU logs. Not in any cold start. | `4c 04 04 00 01 07` | pairing. fw: `eSpiCmdStartPair`, `radio_pair_start_process`, `pair_mode PAIRING/WORK_STATE`, then `radio_evt_cb dev N pair finished`, `radio_pair_success_process`, `switch to WORK_STATE` |
| 16 / 2,3 | `58 04 16 SS 1e` | once per attach (twice back to back when the controller is awake), 1 to 3 attempts per slot | `4c 17 16 SS 13 <19 B serial> CK` (relayed from the controller; only when it is awake) | attach, identity |
| 50 / 2,3 | `58 03 50 SS` | with 0x16 | `4c 0a 50 SS f4 01 a0 a0 8f 10 10 CK` (answered by the headset even if the controller is off) | attach |
| 0a / 2,3 | `58 07 0a SS 00 00 00 10` | with 0x16 | `4c 17 0a SS 00 00 00 10 <16 B radio id> CK` | attach |
| 03 / 2,3 | `58 03 03 SS` | with 0x16 | `4c 06 03 SS 00 08 2d|2e CK` (fw 0.8.45 / 0.8.46) | attach |
| 19 / 0 | `58 03 19 00` | cold start only, twice: t+0.1 s and t+2.7 s (cold1: 2.73, 5.39) | `4c 0b 19 00 ef ff 30 ea ae 0f 00 00 CK`, second `ff ff ...` (byte 0 bit 4 set) | possible; see section 5 |
| 1a / 5 | `58 06 1a 05 01 00 00`; `58 0d 1a 05 02 FPS ...`; `58 0d 1a 05 05 00 ...`; `58 04 1a 05 04` | PC alive, refresh, GPU cfg, close | `4c 04 1a 05 01 07` | no |
| 39 / 0 | `58 04 39 00 RR 00` | every 213 ms (4.7 Hz) | none | no |
| 51 / 0 | `58 19 51 00 01 32 <e0 e0 e1 e1> <g0..g3> 0d 00 .. 28 00 3e 00` | every 80 ms (12.5 Hz) from camera init | `4c 04 51 00 01 07` | camera exposure/gain and LED-frame exposure (40) / gain (62). Not slot addressed |
| 26/27/2a/5c/55/56/5b/29/2c/3a/3c/09/1b/0a(opt 0,5)/03(opt 0,4,5,6,9) | see `analysis/init-sequence.md` | boot, settings | various | no |

Device->host events related to controllers (not commands):
* `4c 0f 37 SS 48 <u32> <batt> 00 02 01 00 08 <fw> CK` every 2 s per slot: the controller is radio-linked/awake but not streaming. Stops once EP 0x82 data flows.
* `4c 04 06 SS 01 07` (one per slot, ~2 ms apart): seen in cold1 at 5.389/5.391 and cold2 at 51.664/51.666, right around the second `0x19` and the moment the early EP 0x82 stream stops. The MCU logs print `cmd 6 opt 2|3 datalen 1` (1626/1503 times in 83 logs, in 17 ms apart bursts). No host OUT with cmd 6 exists in any capture, so it is MCU-generated (M). Meaning unknown (link state change, data 01).

## 4. Timeline at controller connect (Windows)

| Capture | Events |
|---|---|
| cold-start 13:04 (captures2) | 0x37 beacons from 20.66 (controllers already awake). PC alive 52.80, panel on 67.205, `0x55 KK=02` 65.94, **slot 2 burst 67.27 and reports start 67.3**, `0x55 KK=01` 69.0, slot 3 burst 69.29 and reports 69.3 |
| cold1 04:32 | early reports 3.3 to 5.4 s (before any burst), 0x19 at 2.73 and 5.39, `06` events 5.39, 0x37 5.4 to 19.5/21.6, first burst 10.67 (**0x16 unanswered**, 0x50 answered), `0x5b[02 02]` 10.67, `0x55 KK=02` 18.09, **EP 0x82 streams start 19.6 (slot 2) and 21.7 (slot 3)**, bursts 20.29 / 22.38 (answered) |
| cold2 04:36 | early reports 49.6 to 51.7, 0x19 51.7, `06` events 51.66, first burst 56.81 (0x16 unanswered), `0x55 KK=02` 63.99, `KK=01` 67.05, **streams start 68.0 (slot 3), 69.9 (slot 2)**, bursts 68.66 / 70.60 (answered) |
| controllers 12:46 | no cold start; 0x04 StartPair, burst 30 to 40 ms after `pair finished` |

Findings:
* In cold1/cold2 the EP 0x82 stream starts 0.6 to 0.7 s **before** the answered burst. The burst is a reaction (service identity read), not the trigger for streaming. In the 13:04 cold start stream and burst coincide, because the controllers were already awake and the host burst came first.
* In 3 of 3 cold boots the first burst (sent right when `5b [02 02]` is sent, before the `0x55` activation uploads) gets no `0x16` reply; the answered burst follows the activation uploads by 1 to 6 s. Linux gets answers without the uploads, and `etp chip act_s:1` is already set at boot, so activation looks persistent (L for any controller gating).
* Nothing in the headset MCU log (EP 0x84) or the bridge log around any of these moments mentions led, vibrate, motor (motor = IPD lens motor), haptic, rumble or colour. The strings that exist are `PairRx`, `pair_mode`, `radio_*`, `wakeup_dev_check_timer_callback m_wekeup_dev_opt:1 x :y` (60 s timer; `:1`/`:0` flips, meaning unknown), `to radio tx queue full ret = 0` (112 times, radio TX queue overflow in some session).
* Controller report byte 12 is not a state flag: it is the high byte of a 40-bit uptime counter (3 in the 04:24 slam capture, 1 on Linux live, 0 in earlier captures).

## 5. Ranking

### (a) Green light plus one vibration

| # | Candidate | Exact bytes | Evidence | Confidence |
|---|---|---|---|---|
| 1 | No host command: the controller does it on its own when the radio link goes up / the pairing result is confirmed (headset radio firmware may send a link-up indication in the radio payload) | none | no host->device traffic at any connect moment (cold1, cold2, cold-start, controllers) carries a haptic/LED field; MCU logs of 83 sessions show no such command; Windows host attach is a reaction to the stream | M, ~55 % |
| 2 | StartPair per slot | `58 04 04 00 02` (left), `58 04 04 00 03` (right) | only slot-addressed state-changing command; log shows `pair finished`, `WORK_STATE` in the controllers capture, where the user connected controllers; but a normal start never sends it and the controllers need to be in pairing mode (they beacon `AB SS addr` during the 30 s window); it also rebinds the radio link, so it is not harmless | L-M, ~20 % |
| 3 | `0x19` DeviceCheckResult, twice at boot, followed by `06` events for both slots | `58 03 19 00` | the second reply flips flag byte 0 from `ef` to `ff`; `06 SS 01` events appear within 1 to 3 ms in both cold boots; Linux never sends it | L, ~10 % (read-only, harmless to try, but it only looks like a status query) |
| 4 | The attach burst itself (`16`,`50`,`0a`,`03`), with the double `0x16` Windows sends on the answered attempt | `58 04 16 SS 1e` twice in the same ms, then `58 03 50 SS`, `58 07 0a SS 00 00 00 10`, `58 03 03 SS` | Linux sends the same bytes once; effect on LED would be through the radio relay | L, ~5 % |
| 5 | Signed activation uploads `0x5b [01 64]` and `0x55` (KK=02 293 B, KK=01 401 B) | `58 05 5b 05 02 02`, `58 05 5b 05 01 64`, `58 39 55 05 KK II 33 ...` | controller attaches succeed only after them in the cold boots; blob contains the headset serial and a Unix timestamp, RSA/SHA256 verified by firmware, cannot be replayed blindly | L, ~5 % |

### (b) IR tracking LEDs

| # | Candidate | Evidence | Confidence |
|---|---|---|---|
| 1 | No host command: LEDs strobe in sync with the headset camera frame sync (`start_fsin_pwm`, LED frame 25 Hz alternating with tracking frames) once the controller is linked. Linux camera tiles already show bright blobs in LED frames (`analysis/overnight/cam.md`), which suggests the LEDs flash today (not verified to be the controllers; dark room, no hand in view in that test) | no controller-addressed LED command in 83 MCU logs or any capture; Windows LED frame params only arrive via 0x51 (camera-wide) | M, ~60 % |
| 2 | `0x51` push (exposure/gain incl. LED pair 0x28/0x3e) | `58 19 51 00 01 32 AA AA BB BB GG GG GG GG 0d 00 .. 28 00 3e 00`, 12.5 Hz, present in every Windows session; sets how bright the LED frames are exposed, not whether the LEDs light. Already implemented behind `PIMAX_CAM_AE=1` | L-M, ~20 % that it matters for LED visibility |
| 3 | Link-state candidates 2 and 3 of table (a) | same bytes as above | L |

Overall: probability that a still-unknown extra USB command is required for either effect is at most about 25 %. The most informative cheap step is not a new command but a better capture (next section).

## 6. Limits of this analysis

* The p3c MCU logs only record commands the MCU dispatches; commands to the controller that pass through unlogged paths would not show, but the USB captures (which see all of EP 0x01) agree.
* No capture has a marker for "green/vibrate happened". Captures cold1/cold2 do contain controller (re)connects (streams resume at 19.6/21.7 and 68.0/69.9), but it is unknown whether the user saw/felt anything at those moments.
* No capture contains a game or app haptic request, so the haptic command (apps) is still unknown. The connect-time buzz, if host initiated, would have been in cold1/cold2 and is not.
* Windows binaries (`pi_server.exe`, PVR client) are not in the dump; `strings` on the two `pi_server` crash dumps only show the class names `CVrControllerHapticWrapper` / `ICVrControllerHapticWrapperClient`.

## 7. Suggested next steps (need approval where they send something)

1. Windows, read-only: USBPcap capture with Pimax Play running, controllers switched OFF at start, then switch each one on with spoken/typed markers "green now", "buzz now". Compare the host OUT list at those exact seconds (expected: only the burst reaction) and the EP 0x81/0x84 device events (06, 37, radio log lines). Also capture one game with a haptic pulse (e.g. a Steam VR controller test) to learn the app haptic command.
2. Linux, no commands: run `omarchyvr controllers --raw` style listening only (no attach burst) while switching a controller on, log EP 0x81 (`0x37`, `0x06`) and EP 0x84 (`radio_evt`, `PairRx`) and compare with the Windows timeline above. If Linux shows the same `06`/`0x37` pattern and the controller still stays dark, the LED/buzz is gated by something the headset radio does at link-up (e.g. needs `0x19` or pairing mode).
3. Candidates that would be sent only after the user approves each: `58 03 19 00` (status read, lowest risk), then `58 04 04 00 SS` (StartPair, state changing; controllers must be held in pairing mode first). Activation uploads (`0x55`) are not worth trying for this.
