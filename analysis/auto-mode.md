# Auto mode: signals, Windows behaviour, pairing (2026-10-14)

Goal: behave like Pimax Play. Putting the headset on starts the session; leaving it stops the session; the room is
remembered; the controllers pair from the UI; and the user picks the game runtime. Implementation:
`rust/pimax-autod` (user guide: [docs/AUTO-MODE.md](../docs/AUTO-MODE.md); live test:
[auto-mode-live-checklist.md](auto-mode-live-checklist.md)).

Approval tags used below:
- **now**: implemented and used, read-only, no approval needed.
- **probe**: a verified Windows byte sequence. It is only a one-shot live-test tool, and the daemon does not use it.
- **approval**: needs the user's OK.
- **no**: not available.

## 1. Signals

### Headset plugged in
| Signal | How (no root) | Reliability | Latency | Status |
|---|---|---|---|---|
| USB 34a4:0044 present | scan `/sys/bus/usb/devices/*/idVendor,idProduct` every `poll_s` (2 s), the same check as the `pimax vr` preflight (`rust/pimax-tools/src/vr/host.rs` `usb_headset_present`) | high; re-enumeration during a power cycle gives short gaps, so it is debounced (plug 4 s, unplug 6 s) | 2-6 s | **now** |
| hidraw IF0-IF3 present | `pimax::discovery::find_interfaces` (`/sys/class/hidraw/*/device/uevent`) | high | 2 s | now (used to find IF1/IF2) |
| udev netlink monitor | needs a netlink socket or libudev; polling sysfs at 0.5 Hz costs ~nothing | - | <1 s | not needed |
| Camera 34a4:0014, bridge 34a4:0043, eye 2104:0561 | same sysfs scan | they appear later than the HID (power sequencing) | - | not used |

### Headset worn
| Signal | Evidence | Reliability | Latency | Status |
|---|---|---|---|---|
| **IMU micro-motion** (gyro std over a 0.4 s window, hidraw IF2, read-only, no bytes sent) | 14 Oct SLAM recordings, 0.5 s windows (`pimax-autod replay-imu <rec>/euroc_*/mav0/imu0/data.csv`): at rest 0.090-0.10 deg/s std (noise floor), lying on a cushion 0.13-0.18, worn 0.3 and up (typ. 1-60). Thresholds 0.2 / 0.3 with hysteresis. Replays: `162452` still for 8.5 s, then worn/moving 8.5-52 s, then still (put down) to the end; `151524` still for 16.5 s, then worn/moving to the end; no class flapping. | good for "on a head vs lying still". It cannot tell "held in the hand" from "worn", so wear also needs 4 s of continuous evidence. It is blind when the IMU is silent (headset asleep without `pimax-keepalived`). | 4-6 s | **now** (daemon default `imu = true`) |
| Controller input (buttons, touch a/b/stick, trigger/grip > 40), hidraw IF1, read-only | `pimax_proto::decode_controller`. Reports only flow while a controller is attached (in a session). | high as "the user is there" | 2 s | **now** (counts as wear evidence) |
| Controller touch bit t4 (byte 49 bit 4, `rust/pimax-proto/src/controller.rs:58`) | rest value 0x10 left / 0 right; "likely proximity / hand-held sensor" (analysis/controllers-linux.md:62), inferred | unverified | - | not used (could add "controller in hand") |
| **0x2c `eSpiCmdGetVisorStatus`** | request `58 04 2c 00 00` is VERIFIED in the Windows captures: interrupt OUT EP 0x01, reply `4c 04 2c 00 1c 54` on EP 0x81 about 2 ms later. Seen at `captures2/20261013-130457-cold-start-hub2.pcap` frames 41980/41984 and 42586/42588; `pimax-data/windows-20261014/usb-captures/20261014-043157-cold1-hub2.pcap` 14567/14573 and 14647/14651; `20261014-043545-cold2-hub2.pcap` 14185/14191 and 14577/14579; `captures2/20261013-130710-settings-hub2.pcap` 232807/232813. Windows sends it twice per cold start (after the `27/0 BildVer` read) and once after a refresh-rate change, and never while running. **All 6 replies are `1c 54`**, so nothing links it to wear yet (analysis/init-sequence.md:47, confidence L). | unknown | - | **probe**: `pimax-autod visor-probe` sends exactly that request and prints the replies. Live checklist step 3 compares worn vs on the desk. If the reply changes, `Signals::visor` (already in the state machine, where it overrides the IMU) gets a source. |
| Proximity sensor in the regular reports | none decoded. The IMU report has only ts, samples and IPD (`pimax-proto/src/imu.rs`); 0x37 has slot, battery and fw. analysis/hang-worn-debug.md:103-107 ruled out "proximity wakes the panels". | - | - | no |
| Eye tracker pupil presence (`pimax-eye` `direction_valid`, `pupil_mm`) | excellent in principle | single-owner device (docs/LESSONS.md row 3); Monado owns it in `desk --eye`, and before a session the chip must be powered (`3c/5`) | - | no (conflicts with Monado; only one process may open it) |
| Display link (`/sys/class/drm/card1-DP-1/status`) | says `connected` even during a hang (analysis/hang-worn-debug.md:101); does not follow the panel or wear | low | - | not a wear signal |

### In use / idle
| Signal | How | Status |
|---|---|---|
| Head motion | the same IMU burst (works next to Monado: hidraw gives every reader its own copy of each input report) | now |
| Head ring `$XDG_RUNTIME_DIR/pimax-anchor.head` (Monado with `PIMAX_ANCHOR=1`) | pose deltas; only with 6DoF + anchor | not used (the IMU covers it) |
| Controller activity | IF1 burst (above); `pimax-ctrltrack` pose shm `$XDG_RUNTIME_DIR/pimax-ctrltrack.pose` | IF1 now |
| Our session | `monado-service` comm in `/proc` + the launcher's pid files (`/tmp/pimax-vr-session/{passthrough,wayvr}.pid`, live and with the right comm) give desk; Monado without them gives a vr/game session | now |
| OpenXR app activity | `$XDG_RUNTIME_DIR/pimax-perf.stats` (Monado with PIMAX_PERF_FILE; desk default, `vr start --perf`), with a non-overlay `active=1` app, fresh < 2.5 s. In a non-desk session that means a game. | now |
| Steam game | a `/proc/*/cmdline` with `SteamLaunch` and `AppId=` (Steam's reaper wraps every game) | now |
| SteamVR | `vrserver` / `vrcompositor` comm | now |
| Voice | `pimax-voice` socket `level{speech:true}` events | not used (keep the daemon small) |
| Idle | "not worn" = IMU Still and no controller input. **Unknown never counts as idle**, so a silent IMU can never stop a session. | now |

## 2. Pimax Play on Windows (what the captures and docs show)
- No auto-start on wear, no proximity handling and no idle timeout is visible in any capture or doc. The only proximity
  mention is a hypothesis (analysis/research/6dof-ideas-2026.md:94).
- **Power button:** firmware `uart_key power:[1]` leads to `eSpiCmdPanelOnReq`; Windows relays `58 04 29 00 00/01` (panel
  off/on), and the audio amps follow (analysis/poweron.md:93-94). Pimax Play's "sleep" is panel off on the button.
  Auto mode does NOT send 0x29: it is not approved, and the rules allow no display power change. Sleep here means the
  session is stopped.
- **Exit:** `1a/5 [04]`, two `39 [00]`, then the firmware's `pc_heartbeat_timeout` (5 s) and `request_panel_on 0`
  (analysis/poweron.md:80-87). The firmware itself turns the panel off when no host talks to it. Auto mode leaves this
  to `pimax-keepalived` and never touches it.
- `pimax-info.json` (pimax-data/windows-20261014/...) shows `ScreenState_Bool:0` and `SleepMode_Bool:0` for the
  controllers, meaning unknown.

## 3. Pairing (Windows flow, from captures/20261013-124629-controllers-hub2.pcap)
1. Normal starts never pair: the pairing persists in the headset radio (analysis/init-sequence.md:14). StartPair is in
   no cold start.
2. Host `58 04 04 00 SS` (SS 2 = left, 3 = right), opt 0, EP 0x01: frames 59024 (t=41.2 s, SS=2, nobody answered),
   98342 (t=71.7 s, SS=3) and 182552 (t=121.5 s, SS=2). Ack `4c 04 04 00 01 07` (analysis/controller-activation.md:37).
3. Firmware log (IF3): `eSpiCmdStartPair`, `Pairing device = N`, `pair_mode` toggling PAIRING/WORK_STATE about every
   325 ms; the controller beacons `PairRx: len 3 AB <slot> <addr>` (analysis/controllers-settings.md:128-129).
4. Success: `radio_evt_cb dev N pair finished`, then `radio_pair_success_process`, then `switch to WORK_STATE`. Timeout
   or cancel after about 30 s: `radio_pair_cancel_or_timeout_process` (analysis/controllers-settings.md:41,128,130).
5. 30-40 ms later the host attaches the controller: `58 04 16 SS 1e`, `58 03 50 SS`, `58 07 0a SS 00 00 00 10`,
   `58 03 03 SS` (analysis/controllers-linux.md:12-14). Monado's driver already sends the attach burst at start.
6. `58 03 19 00` (DeviceCheckResult) is only in cold starts (cold1 frames 549 and 4792) and is **not** a pairing step.

Implementation (StartPair approved by the user on 2026-10-14; `58 03 19 00` is still NOT approved and is never built):
`rust/pimax-autod/src/pairing.rs` is the UI state machine (Idle -> Waiting(slot, 35 s) -> Paired/Failed), the
firmware-log parser and the send. Send path: the same as the IPD motor (`pimax-eye` `send_motor`): one write-only
whole-report write on hidraw IF0, which coexists with Monado's keepalive writes, so it never takes the HID from a
running session and works without one. The log (IF3) is opened first; if it is unreadable nothing is sent. One send per
request, no retries; success = `pair finished` / `radio_pair_success_process`, else the firmware cancel or our 35 s
timer gives `timeout`. Attach after success: Monado's `pimax_ctrl.c` attach thread sends the attach burst at start
and again when a controller's 0x37 heartbeat shows it awake without reports for 1.5 s (at most every 2.5 s), so a
controller paired in a running session is attached within about 2 s. CLI: `pimax-autod pair left|right`,
`pimax auto pair left|right`, socket command `pair`. The pairing button combo is not in our captures: see Pimax manual.

## 4. Room memory
After a start with 6DoF, when `~/.local/share/pimax/roommap/<room_map>/map.bin` exists, auto mode runs
`pimax-roommap relocalize --source broker --name home --apply` and starts Monado with `PIMAX_ANCHOR=1` (inherited
through the launcher). Auto mode never runs `pimax-anchor apply`, so exactly one writer sets the offset per session,
and Monado deletes the offset file at each start. Relocalisation failure is reported but does not fail the session.
