//! `pimax vr reset-headset`: make the headset power-cycle itself the way it did on Windows after Pimax Play exited
//! (`analysis/poweron.md` section 3.2). Only the captured exit bytes are ever sent: one `cmd 0x1a opt 5 data [04]`,
//! then two keepalives with video rate 0 about 0.3 s apart, then nothing. The firmware's own `pc_heartbeat_timeout`
//! does the rest. The tool then watches the USB bus (no root) for the HID chip (34a4:0044) and the camera
//! (34a4:0014, USB3) to disappear and return; the camera dropping is what makes it a full power cycle.
//!
//! All machine access goes through [`ResetHost`] so the flow is tested with a fake.
use super::host::Clock;
use pimax_proto::{cmd, Report};
use std::fmt::Write as _;
use std::time::Duration;

/// How long the bus is watched after the last frame (default of `--wait`). Measured 2026-10-15: after a USB3 drop the
/// camera came back about 90 s after the reset started, so 60 s was too short.
pub const WATCH: Duration = Duration::from_secs(150);
/// Progress line interval while the camera is still missing.
pub const PROGRESS: Duration = Duration::from_secs(10);
/// Pause between the `0x1a` frame and the first keepalive (the device answers within 50 ms).
pub const REPLY_WAIT: Duration = Duration::from_millis(50);
/// Spacing of the two exit keepalives.
pub const KEEPALIVE_GAP: Duration = Duration::from_millis(300);
/// Bus poll interval.
pub const POLL: Duration = Duration::from_millis(100);
/// How long `pimax-keepalived` may take to become inactive.
pub const STOP_WAIT: Duration = Duration::from_secs(10);

/// After the camera is back, the panel needs until this long after the restart command (Windows capture take 2: the
/// bridge reported `panel change to WORK_WELL` 21.7 s after `58 04 28 00 00`).
pub const PANEL_AFTER: Duration = Duration::from_secs(22);
/// If neither the HID chip nor the camera dropped off USB this long after `58 04 28 00 00`, the headset ignored it
/// (the real one is off USB about 1 s after the command and back after 8 s).
pub const SYS_NO_EFFECT: Duration = Duration::from_secs(15);

/// The headset restart command, the only frame of [`Mode::Sys`]: eSpiCmdSysReset `58 04 28 00 00` (reply
/// `4c 04 28 00 01 07`), captured from Pimax Play's "restart headset" on 2026-10-15 (take 2). The headset is back on
/// USB (HID) after about 8 s and its panel works after about 22 s. Replaces the slow heartbeat timeout of [`exit_frames`].
pub fn sys_frames() -> [Report; 1] {
    [pimax_proto::sys_reset()]
}

/// The three captured frames, in send order.
pub fn exit_frames() -> Result<[Report; 3], String> {
    // keepalive(0) is the exact `58 04 39 00 00 00` of the capture
    Ok([
        pimax_proto::build_command(cmd::ANNOUNCE, 0x05, &[0x04]).map_err(|e| e.to_string())?,
        pimax_proto::keepalive(0),
        pimax_proto::keepalive(0),
    ])
}

/// What the reset needs from the machine.
pub trait ResetHost {
    /// A monado-service process exists (any owner).
    fn monado_running(&self) -> bool;
    /// The session pid file names a live process.
    fn session_active(&self) -> bool;
    /// `systemctl --user stop pimax-keepalived` (blocks until stopped).
    fn stop_keepalived(&mut self) -> Result<(), String>;
    /// `systemctl --user is-active pimax-keepalived` is "active".
    fn keepalived_active(&mut self) -> bool;
    /// `systemctl --user start pimax-keepalived`.
    fn start_keepalived(&mut self) -> Result<(), String>;
    /// Open IF0 (hidraw) for writing and reading.
    fn open_if0(&mut self) -> Result<(), String>;
    /// Write one command frame to IF0.
    fn send(&mut self, frame: &Report) -> Result<(), String>;
    /// Replies read from IF0 since the last call (empty once the node is gone).
    fn drain(&mut self) -> Vec<Report>;
    /// USB device 34a4:`pid` is on the bus.
    fn usb_present(&self, pid: u16) -> bool;
    /// /sys/class/video4linux has a node of the camera 34a4:0014.
    fn camera_node(&self) -> bool {
        self.usb_present(PID_CAM)
    }
    /// Ctrl-C / SIGTERM seen.
    fn interrupted(&self) -> bool;
    /// One line of output.
    fn say(&mut self, line: &str);
    /// Open the display-bridge MCU log (34a4:0043 hidraw, read only). Used by `--mode dp`.
    fn open_bridge_log(&mut self) -> Result<(), String>;
    /// Complete bridge log lines since the last call.
    fn bridge_lines(&mut self) -> Vec<String>;
}

/// HID chip.
pub const PID_HID: u16 = 0x0044;
/// Camera (eYs3d PimaxSensor).
pub const PID_CAM: u16 = 0x0014;

/// Result of the watch.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// Seconds after the last frame the camera disappeared / came back.
    pub cam_gone: Option<f64>,
    pub cam_back: Option<f64>,
    /// Number of HID drop/return events.
    pub hid_drops: u32,
    pub interrupted: bool,
    /// Camera was on the bus at the start.
    pub cam_seen_at_start: bool,
    /// The camera is on USB and has its video4linux node (set together with `cam_back`).
    pub cam_node: bool,
    /// The camera showed up on USB (at this many seconds), whether or not the video node followed.
    pub cam_usb_back: Option<f64>,
    /// How long the watch was allowed to run.
    pub wait: Duration,
    /// [`Mode::Sys`] only: nothing left USB after the command, the headset ignored it.
    pub no_effect: bool,
    /// [`Mode::Sys`] only: the HID chip came back (seconds after the command), `None` if it never left.
    pub hid_back: Option<f64>,
}

impl Outcome {
    /// The camera is usable: on USB, with its video node, after having been gone or absent.
    pub fn camera_recovered(&self) -> bool {
        self.cam_back.is_some() && self.cam_node
    }
}

impl Outcome {
    /// The final line.
    pub fn verdict(&self) -> String {
        let w = self.wait.as_secs();
        if let (Some(g), Some(b)) = (self.cam_gone, self.cam_back) {
            return format!(
                "full power cycle (camera dropped at {g:.1} s, back at {b:.1} s, video node present)"
            );
        }
        if let (false, Some(b)) = (self.cam_seen_at_start, self.cam_back) {
            return format!(
                "camera back after {b:.0} s (it was missing at the start; video node present): recovered, no unplugging needed"
            );
        }
        if self.interrupted {
            return "interrupted before a verdict".into();
        }
        if let Some(g) = self.cam_gone {
            return format!(
                "camera dropped at {g:.1} s and did not return within {w} s (headset powered off? press its button, or run reset-headset again)"
            );
        }
        if !self.cam_seen_at_start {
            if let Some(u) = self.cam_usb_back {
                return format!(
                    "camera on USB at {u:.0} s but /sys/class/video4linux has no 34a4:0014 node after {w} s (uvcvideo not bound?)"
                );
            }
            return format!(
                "camera 34a4:0014 still missing after {w} s: run Restart headset (reset-headset) once more, or reboot; only as the very last resort power-cycle the link box"
            );
        }
        format!("only the HID chip re-enumerated; no full reset within {w} s")
    }
}

fn hex(r: &Report) -> String {
    let n = 2 + r[1] as usize;
    let mut s = String::new();
    for b in r.iter().take(n.min(r.len())) {
        let _ = write!(s, "{b:02x} ");
    }
    s.trim_end().to_string()
}

fn stamp(d: Duration) -> f64 {
    d.as_secs_f64()
}

fn drain_log(h: &mut dyn ResetHost, t0: Duration, clock: &dyn Clock) {
    for r in h.drain() {
        let t = stamp(clock.now() - t0);
        h.say(&format!("  t={t:5.2}  reply  {}", hex(&r)));
    }
}

fn send_and_watch(h: &mut dyn ResetHost, clock: &dyn Clock, wait: Duration, sys: bool) -> Result<Outcome, String> {
    let frames: Vec<(Report, &str)> = if sys {
        sys_frames().into_iter().map(|f| (f, "cmd 0x28 opt 0 data 00 (eSpiCmdSysReset)")).collect()
    } else {
        let [a, k1, k2] = exit_frames()?;
        vec![
            (a, "cmd 0x1a opt 5 data 04"),
            (k1, "cmd 0x39 opt 0 data 00 00 (keepalive, rate 0)"),
            (k2, "cmd 0x39 opt 0 data 00 00 (keepalive, rate 0)"),
        ]
    };
    h.open_if0()?;
    let t0 = clock.now();
    let n = frames.len();
    for (i, (f, name)) in frames.iter().enumerate() {
        h.send(f)?;
        let t = stamp(clock.now() - t0);
        h.say(&format!("  t={t:5.2}  sent   {}   [{name}]", hex(f)));
        let pause = if i == 0 { REPLY_WAIT } else { KEEPALIVE_GAP };
        if sys || i + 1 < n {
            clock.sleep(pause);
            drain_log(h, t0, clock);
        }
    }
    // t0 for the watch: after the last frame
    let ws = clock.now();
    if sys {
        h.say("restart: command sent. The headset restarts itself: it leaves USB for a few seconds, is back on USB after about 8 s and has its picture after about 22 s.");
    } else {
        h.say(&format!(
            "sent the exit sequence; sending nothing more, watching the USB bus ({} s max)",
            wait.as_secs()
        ));
    }
    let mut o = Outcome {
        cam_gone: None,
        cam_back: None,
        hid_drops: 0,
        interrupted: false,
        cam_seen_at_start: h.usb_present(PID_CAM),
        cam_node: false,
        cam_usb_back: None,
        wait,
        no_effect: false,
        hid_back: None,
    };
    let mut hid = h.usb_present(PID_HID);
    let mut cam = o.cam_seen_at_start;
    // waiting for the camera to (re)appear: absent at the start, or gone since
    let mut awaiting = !cam;
    let mut last_progress = Duration::ZERO;
    if !cam {
        h.say("camera 34a4:0014 is not on the bus at the start: waiting for it to come back");
    }
    while clock.now() - ws < wait {
        if h.interrupted() {
            o.interrupted = true;
            h.say("interrupted");
            break;
        }
        drain_log(h, ws, clock);
        let el = clock.now() - ws;
        let t = stamp(el);
        let hid_now = h.usb_present(PID_HID);
        if hid && !hid_now {
            o.hid_drops += 1;
            h.say(&format!("  t={t:5.1}  HID 34a4:0044 gone"));
        } else if !hid && hid_now {
            h.say(&format!("  t={t:5.1}  HID 34a4:0044 back"));
            if sys {
                o.hid_back = Some(t);
            }
        }
        if sys && hid && !hid_now {
            h.say("restart: the headset left USB (it is restarting)");
        } else if sys && !hid && hid_now {
            h.say("restart: the headset is back on USB; waiting for the camera and the picture");
        }
        hid = hid_now;
        let cam_now = h.usb_present(PID_CAM);
        if cam && !cam_now {
            o.cam_gone = Some(t);
            awaiting = true;
            last_progress = el;
            h.say(&format!("  t={t:5.1}  CAMERA 34a4:0014 gone"));
        } else if !cam && cam_now && awaiting {
            o.cam_usb_back = Some(t);
            h.say(&format!("  t={t:5.1}  CAMERA 34a4:0014 on USB"));
        }
        cam = cam_now;
        if sys && o.hid_drops == 0 && o.cam_gone.is_none() && el >= SYS_NO_EFFECT {
            o.no_effect = true;
            h.say("restart: nothing left USB after 15 s: the headset did not act on the restart command");
            break;
        }
        if awaiting && cam_now && h.camera_node() {
            o.cam_back = Some(t);
            o.cam_node = true;
            h.say(&format!("  t={t:5.1}  CAMERA 34a4:0014 back (video node present)"));
            if sys {
                h.say("restart: the camera is back");
                // the panel comes on about 22 s after the command, on its own (no panel-on request is needed)
                while clock.now() - ws < PANEL_AFTER && !h.interrupted() {
                    clock.sleep(POLL);
                }
                h.say("restart: the headset is up (the picture is on by now)");
            }
            break;
        }
        if awaiting && el.saturating_sub(last_progress) >= PROGRESS {
            last_progress = el;
            h.say(&format!(
                "  t={t:5.1}  still waiting for the camera ({} s left)",
                (wait.as_secs_f64() - t).max(0.0).round() as u64
            ));
        }
        clock.sleep(POLL);
    }
    Ok(o)
}

/// Whole flow. The keepalive service is restarted on every path after it was stopped.
pub fn reset_headset(h: &mut dyn ResetHost, clock: &dyn Clock) -> Result<Outcome, String> {
    reset_headset_wait(h, clock, WATCH)
}

/// [`reset_headset`] with an explicit watch time (`--wait`).
pub fn reset_headset_wait(
    h: &mut dyn ResetHost,
    clock: &dyn Clock,
    wait: Duration,
) -> Result<Outcome, String> {
    reset_with(h, clock, wait, false)
}

/// The fast restart: the captured `58 04 28 00 00` (eSpiCmdSysReset), then watch the bus and wait for the picture.
pub fn reset_sys_wait(h: &mut dyn ResetHost, clock: &dyn Clock, wait: Duration) -> Result<Outcome, String> {
    reset_with(h, clock, wait, true)
}

fn reset_with(h: &mut dyn ResetHost, clock: &dyn Clock, wait: Duration, sys: bool) -> Result<Outcome, String> {
    if h.monado_running() {
        return Err("refusing: monado-service is running (stop it with `pimax vr stop`)".into());
    }
    if h.session_active() {
        return Err("refusing: the session pid file names a running VR session".into());
    }
    h.say("stopping pimax-keepalived");
    let mut result = h.stop_keepalived().and_then(|()| {
        let t0 = clock.now();
        while h.keepalived_active() {
            if clock.now() - t0 >= STOP_WAIT {
                return Err("pimax-keepalived is still active after 10 s".to_string());
            }
            clock.sleep(Duration::from_millis(100));
        }
        h.say("pimax-keepalived is inactive");
        Ok(())
    });
    if result.is_ok() {
        match send_and_watch(h, clock, wait, sys) {
            Ok(o) => return finish(h, Ok(o)),
            Err(e) => result = Err(e),
        }
    }
    finish(h, result.map(|()| unreachable_outcome()))
}

fn unreachable_outcome() -> Outcome {
    Outcome {
        cam_gone: None,
        cam_back: None,
        hid_drops: 0,
        interrupted: false,
        cam_seen_at_start: false,
        cam_node: false,
        cam_usb_back: None,
        wait: WATCH,
        no_effect: false,
        hid_back: None,
    }
}

fn finish(h: &mut dyn ResetHost, r: Result<Outcome, String>) -> Result<Outcome, String> {
    h.say("restarting pimax-keepalived");
    if let Err(e) = h.start_keepalived() {
        let msg = format!("could not restart pimax-keepalived: {e}");
        return Err(match r {
            Err(first) => format!("{first}; {msg}"),
            Ok(_) => msg,
        });
    }
    r
}

// ---------------------------------------------------------------------------------------------------------------
// `--mode dp`: power-cycle the display chain (LT7911UXE DP->MIPI bridge, panels) through the firmware's own
// heartbeat timeout. Measured 2026-10-14 (analysis/hang-worn-debug.md, "Software headset reset experiments"):
// ~4.5 s after the last keepalive the bridge logs `pc_alive == 0`, `sony_panel_power_off`, `dp_power_off` (the GPU
// sees an HPD drop: DRM change uevent); after the keepalive (wake `1a/5 01`) returns it logs `dp_power_on`,
// `lt7911uxe_power_on` (second uevent) and, if the GPU still sends video, `calc fps` -> `Panel ON`. Nothing is sent
// that the keepalive daemon does not send anyway. It does NOT clear a stuck NVIDIA display
// (vkAcquireDrmDisplayEXT failing): that state lives in the GPU.

/// How long to wait for `dp_power_off` after the keepalive stopped.
pub const DP_OFF_WAIT: Duration = Duration::from_secs(15);
/// How long to watch for `dp_power_on` / `Panel ON` after the keepalive restarted.
pub const DP_ON_WAIT: Duration = Duration::from_secs(20);

/// Which reset `pimax vr reset-headset` runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The captured restart command `58 04 28 00 00` (about 8 s to HID, 22 s to the picture; default). If the headset
    /// ignores it, falls back to [`Mode::Exit`].
    Sys,
    /// The captured Play exit sequence (heartbeat timeout; slow, the camera came back after ~90 s), then watch USB.
    Exit,
    /// Heartbeat timeout -> bridge DP power cycle (HPD), then restore.
    Dp,
}

impl Mode {
    /// `exit` or `dp`.
    pub fn parse(s: &str) -> Result<Mode, String> {
        match s {
            "sys" => Ok(Mode::Sys),
            "exit" => Ok(Mode::Exit),
            "dp" => Ok(Mode::Dp),
            o => Err(format!("unknown --mode {o} (sys|exit|dp)")),
        }
    }
}

/// Result of `--mode dp`; times in seconds since the keepalive was stopped.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DpOutcome {
    pub dp_off: Option<f64>,
    pub dp_on: Option<f64>,
    pub panel_on: Option<f64>,
    pub interrupted: bool,
}

impl DpOutcome {
    /// The final line.
    pub fn verdict(&self) -> String {
        match (self.dp_off, self.dp_on, self.panel_on) {
            (Some(a), Some(b), Some(c)) => format!(
                "display chain power-cycled (dp_power_off {a:.1} s, dp_power_on {b:.1} s, Panel ON {c:.1} s: the GPU is \
                 still sending video, so a stuck NVIDIA head is NOT cleared by this)"
            ),
            (Some(a), Some(b), None) => format!(
                "display chain power-cycled (dp_power_off {a:.1} s, dp_power_on {b:.1} s); no video from the GPU yet"
            ),
            _ if self.interrupted => "interrupted before a verdict".into(),
            (Some(a), None, _) => format!(
                "dp_power_off at {a:.1} s but no dp_power_on within 20 s of the keepalive restart (check `pimax vr status`)"
            ),
            (None, _, _) => "no dp_power_off seen within 15 s: the bridge did not power-cycle".into(),
        }
    }
}

/// Lines worth echoing from the bridge log.
fn interesting(l: &str) -> bool {
    [
        "dp_power",
        "lt7911uxe_power_on][577",
        "DISPLAY_STATE",
        "pc_alive",
        "Panel O",
        "calc fps",
        "video stable",
        "sony_panel_power",
    ]
    .iter()
    .any(|k| l.contains(k))
}

/// Pump the bridge log until `until` returns true for a line or `limit` passes. Returns the time of that line.
fn watch_bridge(
    h: &mut dyn ResetHost,
    clock: &dyn Clock,
    t0: Duration,
    limit: Duration,
    o: &mut DpOutcome,
    mut until: impl FnMut(&str) -> bool,
) -> Option<f64> {
    let start = clock.now();
    while clock.now() - start < limit {
        if h.interrupted() {
            o.interrupted = true;
            return None;
        }
        for l in h.bridge_lines() {
            let t = stamp(clock.now() - t0);
            if interesting(&l) {
                h.say(&format!("  t={t:5.1}  bridge {l}"));
            }
            if l.contains("dp_power_off") && o.dp_off.is_none() {
                o.dp_off = Some(t);
            }
            if l.contains("dp_power_on") && o.dp_on.is_none() && o.dp_off.is_some() {
                o.dp_on = Some(t);
            }
            if l.contains("Panel ON") && o.panel_on.is_none() && o.dp_on.is_some() {
                o.panel_on = Some(t);
            }
            if until(&l) {
                return Some(t);
            }
        }
        clock.sleep(POLL);
    }
    None
}

/// `--mode dp`. The keepalive service is restarted on every path after it was stopped.
pub fn reset_dp(h: &mut dyn ResetHost, clock: &dyn Clock) -> Result<DpOutcome, String> {
    if h.monado_running() {
        return Err("refusing: monado-service is running (stop it with `pimax vr stop`)".into());
    }
    if h.session_active() {
        return Err("refusing: the session pid file names a running VR session".into());
    }
    h.open_bridge_log()?;
    let _ = h.bridge_lines(); // discard old lines
    h.say("stopping pimax-keepalived (the bridge powers DP off ~5 s after the last heartbeat)");
    let t0 = clock.now();
    let mut o = DpOutcome::default();
    let stopped = h.stop_keepalived().and_then(|()| {
        while h.keepalived_active() {
            if clock.now() - t0 >= STOP_WAIT {
                return Err("pimax-keepalived is still active after 10 s".to_string());
            }
            clock.sleep(Duration::from_millis(100));
        }
        Ok(())
    });
    if stopped.is_ok() {
        watch_bridge(h, clock, t0, DP_OFF_WAIT, &mut o, |l| {
            l.contains("dp_power_off")
        });
        // let DISPLAY_STATE_WAIT_PC_ALIVE settle before the wake comes back
        if o.dp_off.is_some() && !o.interrupted {
            clock.sleep(Duration::from_secs(1));
        }
    }
    h.say("restarting pimax-keepalived");
    let started = h.start_keepalived();
    stopped?;
    started.map_err(|e| format!("could not restart pimax-keepalived: {e}"))?;
    if o.dp_off.is_some() && !o.interrupted {
        // ends at Panel ON; without GPU video only dp_power_on comes
        watch_bridge(h, clock, t0, DP_ON_WAIT, &mut o, |l| l.contains("Panel ON"));
    }
    Ok(o)
}

/// USB device 34a4:`pid` is on the bus under `sysfs` (`bus/usb/devices`).
pub fn usb_present_in(sysfs: &std::path::Path, pid: u16) -> bool {
    let want = format!("{pid:04x}");
    let Ok(rd) = std::fs::read_dir(sysfs.join("bus/usb/devices")) else {
        return false;
    };
    let rd_trim = |p: std::path::PathBuf| std::fs::read_to_string(p).ok().map(|s| s.trim().to_string());
    rd.flatten().any(|e| {
        rd_trim(e.path().join("idVendor")).as_deref() == Some("34a4")
            && rd_trim(e.path().join("idProduct")).as_deref() == Some(want.as_str())
    })
}

/// `class/video4linux` under `sysfs` has a node whose USB device is 34a4:0014 (the node's `device` is the USB
/// interface; its parent carries idVendor / idProduct).
pub fn camera_node_in(sysfs: &std::path::Path) -> bool {
    let Ok(rd) = std::fs::read_dir(sysfs.join("class/video4linux")) else {
        return false;
    };
    let want = format!("{PID_CAM:04x}");
    let rd_trim = |p: std::path::PathBuf| std::fs::read_to_string(p).ok().map(|s| s.trim().to_string());
    rd.flatten().any(|e| {
        let dev = e.path().join("device");
        // the interface directory first (some kernels), then its parent (the USB device)
        [dev.join(".."), dev.clone()].iter().any(|d| {
            rd_trim(d.join("idVendor")).as_deref() == Some("34a4")
                && rd_trim(d.join("idProduct")).as_deref() == Some(want.as_str())
        })
    })
}

/// Camera state for the preflight: `Some(true)` on USB with its video node, `Some(false)` missing, `None` when the
/// headset itself (HID) is absent so the camera cannot be judged.
pub fn camera_ok_in(sysfs: &std::path::Path) -> Option<bool> {
    if !usb_present_in(sysfs, PID_HID) {
        return None;
    }
    Some(usb_present_in(sysfs, PID_CAM) && camera_node_in(sysfs))
}

/// How long to wait for the camera after the dp retry.
pub const DP_RETRY_WAIT: Duration = Duration::from_secs(30);

/// `reset-headset` / auto recovery: the restart (fast command, then the exit reset if ignored), and if the camera is still missing afterwards (and the user did not
/// interrupt) one automatic `--mode dp` retry, then up to [`DP_RETRY_WAIT`] for the camera. Returns the final verdict
/// and whether the camera is usable.
pub fn exit_then_dp_retry(h: &mut dyn ResetHost, clock: &dyn Clock, wait: Duration) -> Result<(String, bool), String> {
    sys_then_exit_then_dp(h, clock, wait, true)
}

/// [`exit_then_dp_retry`] with the choice of the first step: the fast `58 04 28 00 00` restart (`sys`), or the slow
/// exit sequence only. When the headset ignores the fast restart, the exit sequence follows.
pub fn sys_then_exit_then_dp(h: &mut dyn ResetHost, clock: &dyn Clock, wait: Duration, sys: bool) -> Result<(String, bool), String> {
    let mut o = if sys { reset_sys_wait(h, clock, wait)? } else { reset_headset_wait(h, clock, wait)? };
    if sys && o.no_effect {
        h.say("the headset ignored the restart command: using the slow restart (heartbeat timeout, up to a few minutes)");
        o = reset_headset_wait(h, clock, wait)?;
    }
    if o.camera_recovered() {
        return Ok((o.verdict(), true));
    }
    let missing = !o.cam_seen_at_start || o.cam_gone.is_some();
    if o.interrupted || !missing {
        return Ok((o.verdict(), false));
    }
    h.say("camera still missing: one automatic retry with the display-chain reset (--mode dp)");
    match reset_dp(h, clock) {
        Err(e) => {
            h.say(&format!("dp retry failed: {e}"));
            return Ok((o.verdict(), false));
        }
        Ok(d) if d.interrupted => return Ok(("interrupted before a verdict".into(), false)),
        Ok(_) => {}
    }
    let t0 = clock.now();
    while clock.now() - t0 < DP_RETRY_WAIT {
        if h.interrupted() {
            return Ok(("interrupted before a verdict".into(), false));
        }
        if h.usb_present(PID_CAM) && h.camera_node() {
            let t = (clock.now() - t0).as_secs_f64();
            return Ok((format!("camera back {t:.0} s after the dp retry (video node present): recovered"), true));
        }
        clock.sleep(POLL);
    }
    Ok((format!("{}; the dp retry did not bring it back either", o.verdict()), false))
}

/// How long to wait for the HID chip after the camera is back.
pub const HID_SETTLE: Duration = Duration::from_secs(15);

/// What [`auto_recover`] did.
#[derive(Debug, Clone, PartialEq)]
pub enum Recovery {
    /// Camera fine, or the headset (HID) is not on the bus at all: nothing to do here.
    NotNeeded,
    /// The camera was missing and came back; the string is the verdict line.
    Recovered(String),
}

/// `desk start` / `vr start` with the camera needed: if the camera is missing but the HID chip is present, run the
/// same exit-path reset as `reset-headset` (it refuses while monado-service runs or a session pid is live) and wait
/// for the camera up to `wait`. `Err` carries the message that stops the start.
pub fn auto_recover(h: &mut dyn ResetHost, clock: &dyn Clock, wait: Duration) -> Result<Recovery, String> {
    if h.usb_present(PID_CAM) && h.camera_node() {
        return Ok(Recovery::NotNeeded);
    }
    if !h.usb_present(PID_HID) {
        return Ok(Recovery::NotNeeded);
    }
    h.say("camera 34a4:0014 is missing but the headset is on the bus: restarting the headset (no unplugging needed, up to a few minutes; --no-auto-recover skips this)");
    let (verdict, ok) = exit_then_dp_retry(h, clock, wait)?;
    if verdict.starts_with("interrupted") {
        return Err("interrupted while waiting for the camera".into());
    }
    if !ok {
        return Err(format!("camera recovery failed: {verdict}"));
    }
    // the HID chip drops and returns while the headset restarts: let it settle before the preflight looks
    let t0 = clock.now();
    while !h.usb_present(PID_HID) && clock.now() - t0 < HID_SETTLE {
        clock.sleep(POLL);
    }
    Ok(Recovery::Recovered(verdict))
}

/// The real machine.
pub mod real {
    use super::*;
    use crate::vr::host::{Procs, RealHost};
    use std::fs::File;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    pub struct RealReset {
        pub host: RealHost,
        pub pid_file: PathBuf,
        pub if0: Option<File>,
        pub flag: &'static std::sync::atomic::AtomicBool,
        pub bridge: Option<(File, pimax_proto::LineSplitter)>,
    }

    /// Run [`super::auto_recover`] on the real machine. `Ok(())` also when nothing was needed.
    pub fn auto_recover_real(session_dir: &std::path::Path, wait: std::time::Duration) -> Result<(), String> {
        let mut host = RealReset {
            host: RealHost::from_env(),
            pid_file: session_dir.join("monado.pid"),
            if0: None,
            flag: pimax::sys::install_term_flag(),
            bridge: None,
        };
        let clock = crate::vr::host::RealClock::new();
        match super::auto_recover(&mut host, &clock, wait)? {
            super::Recovery::NotNeeded => Ok(()),
            super::Recovery::Recovered(v) => {
                vout!("{} camera recovered: {v}", pimax::sys::local_hms());
                Ok(())
            }
        }
    }

    fn systemctl(args: &[&str]) -> Result<String, String> {
        let o = Command::new("systemctl")
            .arg("--user")
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("systemctl: {e}"))?;
        if o.status.success() {
            Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
        } else {
            Err(format!(
                "systemctl --user {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&o.stderr).trim()
            ))
        }
    }

    impl ResetHost for RealReset {
        fn monado_running(&self) -> bool {
            let Ok(rd) = std::fs::read_dir(&self.host.proc_root) else {
                return false;
            };
            rd.flatten().any(|e| {
                e.file_name()
                    .to_str()
                    .and_then(|n| n.parse::<u32>().ok())
                    .is_some_and(|pid| {
                        self.host.is_alive(pid)
                            && self.host.comm(pid).as_deref()
                                == Some(crate::vr::config::SERVICE_COMM)
                    })
            })
        }
        fn session_active(&self) -> bool {
            crate::vr::session::read_pid(&self.pid_file).is_some_and(|p| self.host.is_alive(p))
        }
        fn stop_keepalived(&mut self) -> Result<(), String> {
            systemctl(&["stop", "pimax-keepalived"]).map(|_| ())
        }
        fn keepalived_active(&mut self) -> bool {
            Command::new("systemctl")
                .args(["--user", "is-active", "--quiet", "pimax-keepalived"])
                .stdin(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(true)
        }
        fn start_keepalived(&mut self) -> Result<(), String> {
            systemctl(&["start", "pimax-keepalived"]).map(|_| ())
        }
        fn open_if0(&mut self) -> Result<(), String> {
            let roots = pimax::discovery::Roots::from_env();
            let node = pimax::discovery::find_interfaces(&roots)[0]
                .clone()
                .ok_or("IF0 hidraw node of 34a4:0044 not found")?;
            self.if0 = Some(
                pimax::hid::open(&node, true).map_err(|e| format!("{}: {e}", node.display()))?,
            );
            Ok(())
        }
        fn send(&mut self, frame: &Report) -> Result<(), String> {
            let f = self.if0.as_ref().ok_or("IF0 not open")?;
            pimax::hid::write_report(f, frame).map_err(|e| format!("write IF0: {e}"))
        }
        fn drain(&mut self) -> Vec<Report> {
            let mut v = Vec::new();
            let Some(f) = self.if0.as_ref() else {
                return v;
            };
            let mut r = [0u8; pimax_proto::REPORT_SIZE];
            loop {
                match pimax::hid::read_report(f, &mut r) {
                    Ok(Some(_)) => v.push(r),
                    Ok(None) => break,
                    Err(_) => {
                        self.if0 = None; // node gone (re-enumeration)
                        break;
                    }
                }
            }
            v
        }
        fn usb_present(&self, pid: u16) -> bool {
            super::usb_present_in(&self.host.sysfs, pid)
        }
        fn camera_node(&self) -> bool {
            super::camera_node_in(&self.host.sysfs)
        }
        fn interrupted(&self) -> bool {
            self.flag.load(std::sync::atomic::Ordering::Relaxed)
        }
        fn say(&mut self, line: &str) {
            vout!("{} {line}", pimax::sys::local_hms());
        }
        fn open_bridge_log(&mut self) -> Result<(), String> {
            let roots = pimax::discovery::Roots::from_env();
            let node = pimax::discovery::find_bridge_log(&roots)
                .ok_or("bridge log hidraw node of 34a4:0043 not found")?;
            let f =
                pimax::hid::open(&node, false).map_err(|e| format!("{}: {e}", node.display()))?;
            self.bridge = Some((f, pimax_proto::LineSplitter::new()));
            Ok(())
        }
        fn bridge_lines(&mut self) -> Vec<String> {
            let mut v = Vec::new();
            let Some((f, split)) = self.bridge.as_mut() else {
                return v;
            };
            let mut r = [0u8; pimax_proto::REPORT_SIZE];
            loop {
                match pimax::hid::read_report(f, &mut r) {
                    Ok(Some(n)) => split.push(&r[..n.min(r.len())], |l| {
                        v.push(String::from_utf8_lossy(l).trim().to_string())
                    }),
                    Ok(None) => break,
                    Err(_) => {
                        self.bridge = None;
                        break;
                    }
                }
            }
            v
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    struct FakeClock(Cell<Duration>);
    impl Clock for FakeClock {
        fn now(&self) -> Duration {
            self.0.get()
        }
        fn sleep(&self, d: Duration) {
            self.0.set(self.0.get() + d);
        }
    }

    /// `cam_gone_at` / `cam_back_at` in seconds since the fake clock started.
    struct Fake<'a> {
        clock: &'a FakeClock,
        monado: bool,
        pid: bool,
        events: Vec<String>,
        sent: Vec<Report>,
        stop_fails: bool,
        send_fails: bool,
        cam_gone_at: Option<f64>,
        cam_back_at: Option<f64>,
        /// camera not on the bus until this time (absent at the start)
        cam_absent_until: Option<f64>,
        /// video node appears only from this time on
        node_at: Option<f64>,
        hid_absent: bool,
        hid_gone: Option<(f64, f64)>,
        active: Cell<bool>,
        stop_checks: Cell<u32>,
        interrupt_at: Option<f64>,
        lines: RefCell<Vec<String>>,
        replied: bool,
        /// (seconds since start, line); emitted once the clock passes the time
        bridge: Vec<(f64, String)>,
        bridge_missing: bool,
    }
    impl<'a> Fake<'a> {
        fn new(clock: &'a FakeClock) -> Self {
            Self {
                clock,
                monado: false,
                pid: false,
                events: vec![],
                sent: vec![],
                stop_fails: false,
                send_fails: false,
                cam_gone_at: None,
                cam_back_at: None,
                cam_absent_until: None,
                node_at: None,
                hid_absent: false,
                hid_gone: None,
                active: Cell::new(true),
                stop_checks: Cell::new(0),
                interrupt_at: None,
                lines: RefCell::new(vec![]),
                replied: false,
                bridge: vec![],
                bridge_missing: false,
            }
        }
        fn t(&self) -> f64 {
            self.clock.now().as_secs_f64()
        }
    }
    impl ResetHost for Fake<'_> {
        fn monado_running(&self) -> bool {
            self.monado
        }
        fn session_active(&self) -> bool {
            self.pid
        }
        fn stop_keepalived(&mut self) -> Result<(), String> {
            self.events.push("stop".into());
            if self.stop_fails {
                return Err("boom".into());
            }
            Ok(())
        }
        fn keepalived_active(&mut self) -> bool {
            let n = self.stop_checks.get() + 1;
            self.stop_checks.set(n);
            n < 3 // inactive on the third check
        }
        fn start_keepalived(&mut self) -> Result<(), String> {
            self.events.push("start".into());
            self.active.set(true);
            Ok(())
        }
        fn open_if0(&mut self) -> Result<(), String> {
            self.events.push("open".into());
            Ok(())
        }
        fn send(&mut self, f: &Report) -> Result<(), String> {
            if self.send_fails {
                return Err("EIO".into());
            }
            self.events.push("send".into());
            self.sent.push(*f);
            Ok(())
        }
        fn drain(&mut self) -> Vec<Report> {
            if !self.replied && !self.sent.is_empty() {
                self.replied = true;
                let mut r = [0u8; 64];
                r[..6].copy_from_slice(&[0x4c, 0x04, 0x1a, 0x05, 0x01, 0x07]);
                return vec![r];
            }
            vec![]
        }
        fn usb_present(&self, pid: u16) -> bool {
            let t = self.t();
            match pid {
                PID_CAM if self.cam_absent_until.is_some_and(|u| t < u) => false,
                PID_CAM => match (self.cam_gone_at, self.cam_back_at) {
                    (Some(g), Some(b)) => !(t >= g && t < b),
                    (Some(g), None) => t < g,
                    _ => true,
                },
                _ if self.hid_absent => false,
                _ => match self.hid_gone {
                    Some((g, b)) => !(t >= g && t < b),
                    None => true,
                },
            }
        }
        fn camera_node(&self) -> bool {
            match self.node_at {
                Some(n) => self.usb_present(PID_CAM) && self.t() >= n,
                None => self.usb_present(PID_CAM),
            }
        }
        fn interrupted(&self) -> bool {
            self.interrupt_at.is_some_and(|i| self.t() >= i)
        }
        fn say(&mut self, line: &str) {
            self.lines.borrow_mut().push(line.to_string());
        }
        fn open_bridge_log(&mut self) -> Result<(), String> {
            if self.bridge_missing {
                return Err("bridge log hidraw node of 34a4:0043 not found".into());
            }
            self.events.push("bridge".into());
            Ok(())
        }
        fn bridge_lines(&mut self) -> Vec<String> {
            let t = self.t();
            let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.bridge)
                .into_iter()
                .partition(|(at, _)| *at <= t);
            self.bridge = later;
            due.into_iter().map(|(_, l)| l).collect()
        }
    }

    /// The bridge lines of the 08:40 run, relative to the keepalive stop; `restart` shifts the power-on half.
    fn measured(restart: f64, video: bool) -> Vec<(f64, String)> {
        let mut v = vec![
            (4.3, "2833458->[E] g_app_state.pc_alive == 0".to_string()),
            (
                4.3,
                "2833458->[I] state DISPLAY_STATE_WORK_ON => DISPLAY_STATE_NONE".into(),
            ),
            (4.5, "2833688->[I] [sony_panel_power_off][288] i".into()),
            (4.6, "2833731->[I] [dp_power_off][1893] i".into()),
            (4.6, "2833813->[I] device manager old:1 pc_alive:0".into()),
            (restart + 0.3, "2840921->[I] eSpiCmdVideoCtrl cmd 1".into()),
            (restart + 0.4, "2841047->[I] [dp_power_on][1882] i".into()),
            (
                restart + 1.5,
                "2842092->[I] state DISPLAY_STATE_WAIT_PC_ALIVE => DISPLAY_STATE_WAIT_VIDEO_STABLE"
                    .into(),
            ),
        ];
        if video {
            v.push((restart + 5.4, "2846095->[I] calc fps 0 => 72".into()));
            v.push((restart + 11.6, "2852297->[I] Panel ON".into()));
        }
        v
    }

    #[test]
    fn mode_parse() {
        assert_eq!(Mode::parse("exit").unwrap(), Mode::Exit);
        assert_eq!(Mode::parse("dp").unwrap(), Mode::Dp);
        assert!(Mode::parse("usb").unwrap_err().contains("exit|dp"));
    }

    #[test]
    fn dp_mode_full_cycle_with_video() {
        let c = clk();
        let mut h = Fake::new(&c);
        // stop at 0, dp_power_off at 4.6, keepalive back at ~5.8 (after the 1 s settle)
        h.bridge = measured(5.8, true);
        let o = reset_dp(&mut h, &c).unwrap();
        assert_eq!(h.events, ["bridge", "stop", "start"]);
        assert!(h.sent.is_empty(), "dp mode sends nothing itself");
        assert!((o.dp_off.unwrap() - 4.6).abs() < 0.2, "{o:?}");
        assert!(o.dp_on.unwrap() > o.dp_off.unwrap());
        assert!(o.panel_on.unwrap() > 15.0);
        assert!(o.verdict().contains("NOT cleared"), "{}", o.verdict());
        let lines = h.lines.borrow().join("\n");
        assert!(
            lines.contains("dp_power_off") && lines.contains("Panel ON"),
            "{lines}"
        );
        // stops at Panel ON, not at the 20 s limit
        assert!(c.now() < Duration::from_secs(19));
    }

    #[test]
    fn dp_mode_without_video_waits_the_limit() {
        let c = clk();
        let mut h = Fake::new(&c);
        h.bridge = measured(5.8, false);
        let o = reset_dp(&mut h, &c).unwrap();
        assert!(o.dp_on.is_some() && o.panel_on.is_none());
        assert!(
            o.verdict().contains("no video from the GPU yet"),
            "{}",
            o.verdict()
        );
    }

    #[test]
    fn dp_mode_no_power_off_restarts_keepalive() {
        let c = clk();
        let mut h = Fake::new(&c);
        let o = reset_dp(&mut h, &c).unwrap();
        assert_eq!(
            o.verdict(),
            "no dp_power_off seen within 15 s: the bridge did not power-cycle"
        );
        assert_eq!(h.events, ["bridge", "stop", "start"]);
        // no second watch when nothing went off
        assert!(c.now() < Duration::from_secs(16));
    }

    #[test]
    fn dp_mode_refusals_and_errors() {
        let c = clk();
        let mut h = Fake::new(&c);
        h.monado = true;
        assert!(reset_dp(&mut h, &c).unwrap_err().starts_with("refusing"));
        assert!(h.events.is_empty());

        let c = clk();
        let mut h = Fake::new(&c);
        h.bridge_missing = true;
        assert!(reset_dp(&mut h, &c).unwrap_err().contains("0043"));
        assert!(
            h.events.is_empty(),
            "keepalive untouched when the log can't be opened"
        );

        let c = clk();
        let mut h = Fake::new(&c);
        h.stop_fails = true;
        assert!(reset_dp(&mut h, &c).unwrap_err().contains("boom"));
        assert_eq!(h.events, ["bridge", "stop", "start"]);
    }

    #[test]
    fn dp_mode_ctrl_c_restarts_keepalive() {
        let c = clk();
        let mut h = Fake::new(&c);
        h.bridge = measured(5.8, true);
        h.interrupt_at = Some(2.0);
        let o = reset_dp(&mut h, &c).unwrap();
        assert!(o.interrupted);
        assert_eq!(o.verdict(), "interrupted before a verdict");
        assert_eq!(h.events.last().unwrap(), "start");
    }

    fn clk() -> FakeClock {
        FakeClock(Cell::new(Duration::ZERO))
    }

    #[test]
    fn frames_are_byte_exact() {
        let [a, k1, k2] = exit_frames().unwrap();
        assert_eq!(&a[..6], &[0x58, 0x04, 0x1a, 0x05, 0x04, 0x00]);
        assert_eq!(&k1[..6], &[0x58, 0x04, 0x39, 0x00, 0x00, 0x00]);
        assert_eq!(k1, k2);
        assert!(a[6..].iter().all(|&b| b == 0) && k1[6..].iter().all(|&b| b == 0));
        // the normal keepalive differs only in the rate byte
        let normal = pimax_proto::keepalive(0x48);
        assert_eq!(&normal[..6], &[0x58, 0x04, 0x39, 0x00, 0x48, 0x00]);
        assert_eq!(k1[..4], normal[..4]);
    }

    #[test]
    fn order_timing_and_full_cycle() {
        let c = clk();
        let mut h = Fake::new(&c);
        h.hid_gone = Some((7.0, 8.0));
        h.cam_gone_at = Some(20.0);
        h.cam_back_at = Some(33.0);
        let o = reset_headset(&mut h, &c).unwrap();
        assert_eq!(h.events, ["stop", "open", "send", "send", "send", "start"]);
        let [a, k, _] = exit_frames().unwrap();
        assert_eq!(h.sent[0], a);
        assert_eq!(h.sent[1], k);
        assert_eq!(h.sent[2], k);
        assert_eq!(h.stop_checks.get(), 3);
        assert_eq!(o.hid_drops, 1);
        assert!(o.cam_gone.unwrap() >= 19.0 && o.cam_back.unwrap() >= 32.0);
        assert!(o
            .verdict()
            .starts_with("full power cycle (camera dropped at"));
        let lines = h.lines.borrow().join("\n");
        assert!(lines.contains("58 04 1a 05 04 00"), "{lines}");
        assert!(lines.contains("4c 04 1a 05 01 07"), "{lines}");
        assert!(lines.contains("HID 34a4:0044 gone") && lines.contains("CAMERA 34a4:0014 back"));
        // ended early, long before 60 s
        assert!(c.now() < Duration::from_secs(40));
    }

    #[test]
    fn sys_frame_is_the_captured_restart_command() {
        // windows-capture-20261015.md take 2, t=8.722: 58 04 28 00 00
        let [f] = sys_frames();
        assert_eq!(&f[..6], &[0x58, 0x04, 0x28, 0x00, 0x00, 0x00]);
        assert!(f[5..].iter().all(|&b| b == 0));
        assert_eq!(&pimax_proto::to_write_buf(&f)[..6], &[0x00, 0x58, 0x04, 0x28, 0x00, 0x00]);
        assert_eq!(Mode::parse("sys"), Ok(Mode::Sys));
        assert!(Mode::parse("nope").unwrap_err().contains("sys|exit|dp"));
    }

    #[test]
    fn sys_restart_flow_sends_one_frame_and_waits_for_the_picture() {
        // Windows take 2: off USB ~1 s after the command, HID back after 7.8 s, camera ~8.4 s later
        let c = clk();
        let mut h = Fake::new(&c);
        h.hid_gone = Some((1.0, 8.0));
        h.cam_gone_at = Some(1.5);
        h.cam_back_at = Some(17.0);
        let o = reset_sys_wait(&mut h, &c, WATCH).unwrap();
        assert_eq!(h.events, ["stop", "open", "send", "start"]);
        assert_eq!(h.sent, sys_frames().to_vec());
        assert_eq!(o.hid_drops, 1);
        assert!(!o.no_effect && o.camera_recovered());
        assert!(o.hid_back.unwrap() >= 7.5, "{:?}", o.hid_back);
        assert!(o.verdict().starts_with("full power cycle (camera dropped at"), "{}", o.verdict());
        // it waited for the panel (22 s after the command) but nowhere near the old 90 s
        assert!(c.now() >= PANEL_AFTER && c.now() < Duration::from_secs(30), "{:?}", c.now());
        let lines = h.lines.borrow().join("\n");
        for want in [
            "58 04 28 00 00",
            "restart: command sent",
            "restart: the headset left USB",
            "restart: the headset is back on USB",
            "restart: the camera is back",
            "restart: the headset is up",
        ] {
            assert!(lines.contains(want), "{want} missing in:\n{lines}");
        }
    }

    #[test]
    fn sys_restart_is_refused_while_a_session_runs() {
        let c = clk();
        let mut h = Fake::new(&c);
        h.monado = true;
        assert!(reset_sys_wait(&mut h, &c, WATCH).unwrap_err().starts_with("refusing"));
        let mut h = Fake::new(&c);
        h.pid = true;
        assert!(sys_then_exit_then_dp(&mut h, &c, WATCH, true).unwrap_err().starts_with("refusing"));
        assert!(h.events.is_empty() && h.sent.is_empty());
    }

    #[test]
    fn ignored_sys_restart_falls_back_to_the_exit_sequence() {
        let c = clk();
        let mut h = Fake::new(&c);
        // nothing leaves USB after the sys command; the exit sequence then brings a full cycle
        let o = reset_sys_wait(&mut h, &c, WATCH).unwrap();
        assert!(o.no_effect && !o.camera_recovered());
        assert!(c.now() < Duration::from_secs(17), "{:?}", c.now());
        let c = clk();
        let mut h = Fake::new(&c);
        let (_, _) = sys_then_exit_then_dp(&mut h, &c, Duration::from_secs(20), true).unwrap();
        let [a, k, _] = exit_frames().unwrap();
        assert_eq!(h.sent[0], sys_frames()[0]);
        assert_eq!(&h.sent[1..4], &[a, k, k], "exit frames follow the ignored command");
        let lines = h.lines.borrow().join("\n");
        assert!(lines.contains("did not act on the restart command") && lines.contains("ignored the restart command"), "{lines}");
    }

    #[test]
    fn hid_only_runs_the_full_watch() {
        let c = clk();
        let mut h = Fake::new(&c);
        h.hid_gone = Some((5.4, 6.0));
        let o = reset_headset(&mut h, &c).unwrap();
        assert_eq!(
            o.verdict(),
            "only the HID chip re-enumerated; no full reset within 150 s"
        );
        assert!(c.now() >= Duration::from_secs(150));
        assert_eq!(h.events.last().unwrap(), "start");
    }

    #[test]
    fn camera_gone_but_not_back() {
        let c = clk();
        let mut h = Fake::new(&c);
        h.cam_gone_at = Some(10.0);
        let o = reset_headset(&mut h, &c).unwrap();
        assert!(o.verdict().contains("did not return within 150 s"));
    }

    #[test]
    fn camera_absent_at_start_is_tracked_and_reported_back() {
        // 2026-10-15: the camera re-enumerated by itself ~90 s after the reset started
        let c = clk();
        let mut h = Fake::new(&c);
        h.cam_absent_until = Some(90.0);
        h.hid_gone = Some((5.0, 10.0));
        let o = reset_headset(&mut h, &c).unwrap();
        assert!(o.camera_recovered() && !o.cam_seen_at_start);
        let b = o.cam_back.unwrap();
        assert!((89.0..92.0).contains(&b), "{b}");
        assert!(o.verdict().starts_with("camera back after 9"), "{}", o.verdict());
        let lines = h.lines.borrow().join("\n");
        assert!(lines.contains("still waiting for the camera"), "{lines}");
        assert!(lines.contains("video node present"), "{lines}");
        // stops at once, long before the 150 s limit; keepalived restarted
        assert!(c.now() < Duration::from_secs(95));
        assert_eq!(h.events.last().unwrap(), "start");
    }

    #[test]
    fn camera_never_back_waits_the_whole_time_and_says_so() {
        let c = clk();
        let mut h = Fake::new(&c);
        h.cam_absent_until = Some(1e9);
        let o = reset_headset(&mut h, &c).unwrap();
        assert!(!o.camera_recovered());
        assert!(c.now() >= Duration::from_secs(150));
        assert!(o.verdict().contains("still missing after 150 s"), "{}", o.verdict());
    }

    #[test]
    fn failed_exit_reset_retries_once_with_dp_and_never_mentions_unplugging() {
        let c = clk();
        let mut h = Fake::new(&c);
        h.cam_absent_until = Some(1e9);
        let (v, ok) = exit_then_dp_retry(&mut h, &c, Duration::from_secs(20)).unwrap();
        assert!(!ok);
        assert!(h.events.iter().filter(|e| *e == "bridge").count() == 1, "{:?}", h.events);
        assert!(v.contains("dp retry did not bring it back"), "{v}");
        assert!(v.contains("power-cycle the link box") && !v.contains("junction") && !v.contains("unplug"), "{v}");
        assert_eq!(h.events.iter().filter(|e| *e == "start").count(), 3, "keepalived restarted after the sys, exit and dp steps");

        // the camera shows up after the dp retry
        let c = clk();
        let mut h = Fake::new(&c);
        h.cam_absent_until = Some(40.0);
        let (v, ok) = exit_then_dp_retry(&mut h, &c, Duration::from_secs(20)).unwrap();
        assert!(ok && v.contains("after the dp retry"), "{v}");

        // a recovered camera needs no retry
        let c = clk();
        let mut h = Fake::new(&c);
        h.cam_absent_until = Some(5.0);
        let (_, ok) = exit_then_dp_retry(&mut h, &c, Duration::from_secs(20)).unwrap();
        assert!(ok && !h.events.contains(&"bridge".to_string()));
    }

    #[test]
    fn wait_is_configurable() {
        let c = clk();
        let mut h = Fake::new(&c);
        h.cam_absent_until = Some(1e9);
        let o = reset_headset_wait(&mut h, &c, Duration::from_secs(20)).unwrap();
        assert!(o.verdict().contains("after 20 s"), "{}", o.verdict());
        assert!(c.now() >= Duration::from_secs(20) && c.now() < Duration::from_secs(25));
    }

    #[test]
    fn camera_on_usb_but_no_video_node_is_not_recovered() {
        let c = clk();
        let mut h = Fake::new(&c);
        h.cam_absent_until = Some(30.0);
        h.node_at = Some(1e9);
        let o = reset_headset_wait(&mut h, &c, Duration::from_secs(60)).unwrap();
        assert!(!o.camera_recovered());
        assert!(o.verdict().contains("no 34a4:0014 node"), "{}", o.verdict());
    }

    #[test]
    fn video_node_may_lag_the_usb_device() {
        let c = clk();
        let mut h = Fake::new(&c);
        h.cam_absent_until = Some(30.0);
        h.node_at = Some(34.0);
        let o = reset_headset(&mut h, &c).unwrap();
        assert!(o.camera_recovered());
        assert!(o.cam_back.unwrap() >= 33.0 && o.cam_usb_back.unwrap() < 31.0);
    }

    #[test]
    fn auto_recover_does_nothing_when_the_camera_is_there_or_the_headset_is_not() {
        let c = clk();
        let mut h = Fake::new(&c);
        assert_eq!(auto_recover(&mut h, &c, WATCH).unwrap(), Recovery::NotNeeded);
        assert!(h.events.is_empty());
        let mut h = Fake::new(&c);
        h.cam_absent_until = Some(1e9);
        h.hid_absent = true;
        assert_eq!(auto_recover(&mut h, &c, WATCH).unwrap(), Recovery::NotNeeded);
        assert!(h.events.is_empty() && h.sent.is_empty());
    }

    #[test]
    fn auto_recover_runs_the_exit_reset_and_waits() {
        let c = clk();
        let mut h = Fake::new(&c);
        h.cam_absent_until = Some(60.0);
        match auto_recover(&mut h, &c, WATCH).unwrap() {
            Recovery::Recovered(v) => assert!(v.starts_with("camera back after"), "{v}"),
            r => panic!("{r:?}"),
        }
        // the fake headset ignores the sys command (nothing leaves USB), so the exit sequence follows
        assert_eq!(h.events, ["stop", "open", "send", "start", "stop", "open", "send", "send", "send", "start"]);
        let [a, k, _] = exit_frames().unwrap();
        assert_eq!(h.sent, vec![sys_frames()[0], a, k, k], "only captured frames: sys restart, then the exit frames");
    }

    #[test]
    fn auto_recover_refuses_while_monado_runs_and_fails_when_the_camera_stays_away() {
        let c = clk();
        let mut h = Fake::new(&c);
        h.cam_absent_until = Some(1e9);
        h.monado = true;
        let e = auto_recover(&mut h, &c, WATCH).unwrap_err();
        assert!(e.starts_with("refusing"), "{e}");
        assert!(h.events.is_empty() && h.sent.is_empty());

        let c = clk();
        let mut h = Fake::new(&c);
        h.cam_absent_until = Some(1e9);
        let e = auto_recover(&mut h, &c, Duration::from_secs(30)).unwrap_err();
        assert!(e.starts_with("camera recovery failed") && e.contains("still missing"), "{e}");
        assert_eq!(h.events.last().unwrap(), "start");
    }

    /// A fake sysfs tree in a temp dir (never the real /sys).
    fn fake_sys(tag: &str, hid: bool, cam: bool, node: bool) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("pimax-reset-test-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let usb = root.join("bus/usb/devices");
        std::fs::create_dir_all(&usb).unwrap();
        let mk = |name: &str, pid: &str| {
            let d = usb.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("idVendor"), "34a4\n").unwrap();
            std::fs::write(d.join("idProduct"), format!("{pid}\n")).unwrap();
            d
        };
        if hid {
            mk("2-4.1", "0044");
        }
        if cam {
            let d = mk("2-4.4", "0014");
            if node {
                let iface = d.join("2-4.4:1.0");
                std::fs::create_dir_all(&iface).unwrap();
                let v = root.join("class/video4linux/video0");
                std::fs::create_dir_all(&v).unwrap();
                std::os::unix::fs::symlink(&iface, v.join("device")).unwrap();
            }
        }
        root
    }

    /// The layout of the real machine (read from /sys on 2026-10-15), copied into a temp tree: bus/usb/devices and
    /// class/video4linux hold RELATIVE symlinks into devices/..., and video0/device -> ../../../2-4.4:1.0.
    #[test]
    fn real_sysfs_layout_is_detected() {
        let root = std::env::temp_dir().join(format!("pimax-reset-real-layout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let usb2 = root.join("devices/pci0000:00/0000:00:14.0/usb2");
        let dev = usb2.join("2-4/2-4.4");
        let iface = dev.join("2-4.4:1.0");
        let v0 = iface.join("video4linux/video0");
        std::fs::create_dir_all(&v0).unwrap();
        std::fs::write(dev.join("idVendor"), "34a4\n").unwrap();
        std::fs::write(dev.join("idProduct"), "0014\n").unwrap();
        let hid = root.join("devices/pci0000:00/0000:00:14.0/usb1/1-5/1-5.1/1-5.1.4/1-5.1.4.1");
        std::fs::create_dir_all(&hid).unwrap();
        std::fs::write(hid.join("idVendor"), "34a4\n").unwrap();
        std::fs::write(hid.join("idProduct"), "0044\n").unwrap();
        std::fs::create_dir_all(root.join("bus/usb/devices")).unwrap();
        std::fs::create_dir_all(root.join("class/video4linux")).unwrap();
        use std::os::unix::fs::symlink;
        symlink("../../../devices/pci0000:00/0000:00:14.0/usb2/2-4/2-4.4", root.join("bus/usb/devices/2-4.4")).unwrap();
        symlink("../../../devices/pci0000:00/0000:00:14.0/usb1/1-5/1-5.1/1-5.1.4/1-5.1.4.1", root.join("bus/usb/devices/1-5.1.4.1")).unwrap();
        symlink("../../devices/pci0000:00/0000:00:14.0/usb2/2-4/2-4.4/2-4.4:1.0/video4linux/video0", root.join("class/video4linux/video0")).unwrap();
        symlink("../../../2-4.4:1.0", v0.join("device")).unwrap();
        assert!(usb_present_in(&root, PID_CAM) && usb_present_in(&root, PID_HID));
        assert!(camera_node_in(&root));
        assert_eq!(camera_ok_in(&root), Some(true));
        // unplugged: the device directory and its symlink vanish
        std::fs::remove_file(root.join("bus/usb/devices/2-4.4")).unwrap();
        assert_eq!(camera_ok_in(&root), Some(false));
    }

    #[test]
    fn fake_sysfs_camera_states() {
        let r = fake_sys("ok", true, true, true);
        assert!(usb_present_in(&r, PID_CAM) && usb_present_in(&r, PID_HID) && camera_node_in(&r));
        assert_eq!(camera_ok_in(&r), Some(true));
        let r = fake_sys("nonode", true, true, false);
        assert!(usb_present_in(&r, PID_CAM) && !camera_node_in(&r));
        assert_eq!(camera_ok_in(&r), Some(false));
        let r = fake_sys("nocam", true, false, false);
        assert!(!usb_present_in(&r, PID_CAM));
        assert_eq!(camera_ok_in(&r), Some(false));
        let r = fake_sys("nohid", false, false, false);
        assert_eq!(camera_ok_in(&r), None, "headset absent: cannot judge the camera");
        assert_eq!(camera_ok_in(&r.join("nothing-here")), None);
        // a video node of some other device does not count
        let r = fake_sys("other", true, false, false);
        let other = r.join("bus/usb/devices/1-1");
        std::fs::create_dir_all(other.join("1-1:1.0")).unwrap();
        std::fs::write(other.join("idVendor"), "046d\n").unwrap();
        std::fs::write(other.join("idProduct"), "0014\n").unwrap();
        let v = r.join("class/video4linux/video0");
        std::fs::create_dir_all(&v).unwrap();
        std::os::unix::fs::symlink(other.join("1-1:1.0"), v.join("device")).unwrap();
        assert!(!camera_node_in(&r));
    }

    #[test]
    fn refuses_while_monado_runs_or_session_active_without_touching_anything() {
        for (m, p) in [(true, false), (false, true)] {
            let c = clk();
            let mut h = Fake::new(&c);
            h.monado = m;
            h.pid = p;
            let e = reset_headset(&mut h, &c).unwrap_err();
            assert!(e.starts_with("refusing"), "{e}");
            assert!(h.events.is_empty() && h.sent.is_empty());
        }
    }

    #[test]
    fn keepalive_is_restarted_on_every_error_path() {
        let c = clk();
        let mut h = Fake::new(&c);
        h.stop_fails = true;
        assert!(reset_headset(&mut h, &c).unwrap_err().contains("boom"));
        assert_eq!(h.events, ["stop", "start"]);
        assert!(h.sent.is_empty());

        let c = clk();
        let mut h = Fake::new(&c);
        h.send_fails = true;
        assert!(reset_headset(&mut h, &c).unwrap_err().contains("EIO"));
        assert_eq!(h.events, ["stop", "open", "start"]);
    }

    #[test]
    fn ctrl_c_stops_the_watch_and_restarts_keepalived() {
        let c = clk();
        let mut h = Fake::new(&c);
        h.interrupt_at = Some(3.0);
        let o = reset_headset(&mut h, &c).unwrap();
        assert!(o.interrupted);
        assert_eq!(o.verdict(), "interrupted before a verdict");
        assert_eq!(h.events.last().unwrap(), "start");
        assert!(c.now() < Duration::from_secs(10));
    }
}
